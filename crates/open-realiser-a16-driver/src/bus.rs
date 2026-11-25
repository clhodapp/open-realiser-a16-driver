// SPDX-License-Identifier: MIT
//! The engine's [`Bus`] on [`Usb`]: transfers, control requests, the interfaces, and how a
//! libusb status reaches the engine. Every call runs on the USB thread, cancellation
//! included.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use a16_engine::{
    Bus, BusError, ControlSetup, Direction, Engine, INTERFACES, Logger, PcmRing, Status, Transfer,
    UsbEndpoint,
};

use crate::usb::{Completion, TRANSFER_COMPLETED, TransferKind, Usb, UsbError};

/// A PCM transfer that has not completed after this long fails the stream.
const TRANSFER_TIMEOUT: Duration = Duration::from_millis(2000);
/// A gap this long between two completed PCM transfers in one direction is logged, with
/// how the USB thread spent it.
const GAP_NS: u64 = 20_000_000;
/// A control request that has not completed after this long fails the bus.
const CONTROL_TIMEOUT: Duration = Duration::from_millis(2000);
/// How long a cancellation may take to drain before the process exits.
const DRAIN_LIMIT_NS: u64 = 5_000_000_000;

const LIBUSB_TRANSFER_TYPE_BULK: u8 = 2;
const LIBUSB_TRANSFER_TYPE_INTERRUPT: u8 = 3;

/// Which component the engine's next log line comes from: the endpoint binding comes from
/// Ozzy's generic engine, everything else from the Ploytec engine.
static OZZY_COMPONENT: AtomicBool = AtomicBool::new(false);

/// An engine logger that writes each line to `sink` with the prefix of the component it
/// comes from, `[Ozzy] ` or `[Ploytec] `.
pub fn engine_logger(mut sink: impl FnMut(&str) + Send + 'static) -> Logger {
    Box::new(move |line| {
        let prefix = if OZZY_COMPONENT.load(Ordering::Relaxed) {
            "[Ozzy] "
        } else {
            "[Ploytec] "
        };
        sink(&format!("{prefix}{line}"));
    })
}

/// What the bus logs through: one line per call, without a newline.
pub type Log = Box<dyn FnMut(&str)>;

/// Log to stderr through the logger thread of [`crate::log`], which never blocks the caller.
pub fn stderr_log() -> Log {
    Box::new(crate::log::line)
}

/// The libusb bus of one open A16.
pub struct UsbBus<U> {
    pub usb: U,
    pending: u32,
    types: [u8; 256],
    claimed: [bool; 256],
    detached: [bool; 256],
    pub config_value: u8,
    /// Per direction, out then in, the ring position the engine last reported.
    pub progress: [u64; 2],
    /// Per direction, the transfers that completed, and their bytes.
    pub completed: [u64; 2],
    pub bytes: [u64; 2],
    /// Short input transfers, passed on and silenced.
    pub short_in: u64,
    draining: bool,
    pub failed: bool,
    input_ready: bool,
    timing: Timing,
    /// The transfers each poll returns, kept so that its capacity is reused.
    completions: Vec<Completion>,
    trace: Option<Box<dyn Write>>,
    log: Log,
}

/// How the USB thread spends the time between completed PCM transfers, for the gap log.
#[derive(Clone, Copy, Debug, Default)]
struct Timing {
    /// Per direction, when the last PCM transfer completed; 0 before the first.
    last: [u64; 2],
    /// When the last event wait returned; 0 before the first.
    returned: u64,
    /// Since the last completion of either direction: the longest event wait, and the
    /// longest stretch between two waits.
    wait: u64,
    outside: u64,
}

fn dir_index(direction: Direction) -> usize {
    match direction {
        Direction::Out => 0,
        Direction::In => 1,
    }
}

impl<U: Usb> UsbBus<U> {
    /// A bus on `usb`, logging to `log`, and with `trace` writing each submission,
    /// completion and request there instead.
    pub fn new(usb: U, trace: Option<Box<dyn Write>>, log: Log) -> UsbBus<U> {
        UsbBus {
            usb,
            pending: 0,
            types: [0; 256],
            claimed: [false; 256],
            detached: [false; 256],
            config_value: 0,
            progress: [0; 2],
            completed: [0; 2],
            bytes: [0; 2],
            short_in: 0,
            draining: false,
            failed: false,
            input_ready: false,
            timing: Timing::default(),
            completions: Vec::with_capacity(32),
            trace,
            log,
        }
    }

    /// The state of a fresh bus, for a newly opened handle.
    pub fn reset_state(&mut self) {
        self.pending = 0;
        self.types = [0; 256];
        self.claimed = [false; 256];
        self.detached = [false; 256];
        self.config_value = 0;
        self.progress = [0; 2];
        self.completed = [0; 2];
        self.bytes = [0; 2];
        self.short_in = 0;
        self.draining = false;
        self.failed = false;
        self.input_ready = false;
        self.timing = Timing::default();
    }

    /// Transfers in flight.
    pub fn pending(&self) -> u32 {
        self.pending
    }

    /// Mark `endpoint` as one transfers may go to, of the `bmAttributes` type.
    pub fn set_endpoint_type(&mut self, endpoint: u8, attributes: u8) {
        self.types[usize::from(endpoint)] = attributes & 3;
    }

    fn log(&mut self, args: std::fmt::Arguments<'_>) {
        (self.log)(&args.to_string());
    }

    /// A line for the trace, or for the log without a trace.
    fn trace_or_log(&mut self, args: std::fmt::Arguments<'_>) {
        match self.trace.as_mut() {
            Some(trace) => {
                let _ = writeln!(trace, "{args}");
            }
            None => (self.log)(&args.to_string()),
        }
    }

    fn trace_only(&mut self, args: std::fmt::Arguments<'_>) {
        if let Some(trace) = self.trace.as_mut() {
            let _ = writeln!(trace, "{args}");
        }
    }

    /// Flush the trace.
    pub fn finish_trace(&mut self) -> std::io::Result<()> {
        match self.trace.take() {
            Some(mut trace) => trace.flush(),
            None => Ok(()),
        }
    }

    /// Claim the engine's interfaces at their alternate settings and bind the engine to
    /// their PCM endpoints. With `detach`, a kernel driver on an interface is detached
    /// first; without, one fails the configuration.
    pub fn configure(&mut self, engine: &mut Engine, detach: bool) -> bool {
        let config = match self.usb.active_config() {
            Ok(config) => config,
            Err(error) => {
                self.log(format_args!("active configuration: {error}"));
                return false;
            }
        };
        self.config_value = config.value;
        let mut endpoints = Vec::new();
        for setting in INTERFACES {
            let (number, alt) = (setting.number, setting.alt_setting);
            let Some(desc) = config
                .settings
                .iter()
                .rfind(|d| d.interface == number && d.alt_setting == alt)
            else {
                self.log(format_args!("missing interface {number} alt {alt}"));
                return false;
            };
            if let Err(error) = self.claim(number, alt, detach) {
                if let Some(error) = error {
                    self.log(format_args!("configure: {error}"));
                }
                return false;
            }
            for ep in &desc.endpoints {
                if endpoints.len() == 32 {
                    self.log(format_args!("too many endpoints"));
                    return false;
                }
                endpoints.push(UsbEndpoint {
                    interface: number,
                    address: ep.address,
                    attributes: ep.attributes,
                    interval: ep.interval,
                    max_packet: ep.max_packet_size,
                });
                self.set_endpoint_type(ep.address, ep.attributes);
            }
        }
        OZZY_COMPONENT.store(true, Ordering::Relaxed);
        let bound = engine.bind_endpoints(&endpoints);
        OZZY_COMPONENT.store(false, Ordering::Relaxed);
        for (role, direction) in Direction::ALL.into_iter().enumerate() {
            let supported = bound[direction].is_some_and(|ep| {
                ep.attributes & 3 >= 2
                    && (ep.max_packet == 512
                        || ep.max_packet
                            == match direction {
                                Direction::In => 1024,
                                Direction::Out => 964,
                            })
            });
            if !supported {
                self.log(format_args!(
                    "missing or unsupported PCM endpoint layout (role {role})"
                ));
                return false;
            }
        }
        true
    }

    /// Claim interface `number` and select `alt`: Err(None) when a kernel driver holds it
    /// without `detach`, Err(Some) on a libusb error.
    fn claim(&mut self, number: u8, alt: u8, detach: bool) -> Result<(), Option<UsbError>> {
        let i = usize::from(number);
        match self.usb.kernel_driver_active(number) {
            Ok(true) => {
                if !detach {
                    self.log(format_args!(
                        "interface {number} has a driver; use --detach explicitly"
                    ));
                    return Err(None);
                }
                self.usb.detach_kernel_driver(number).map_err(Some)?;
                self.detached[i] = true;
            }
            Ok(false) => {}
            Err(error) if error == UsbError::NOT_SUPPORTED => {}
            Err(error) => return Err(Some(error)),
        }
        self.usb.claim_interface(number).map_err(Some)?;
        self.claimed[i] = true;
        self.trace_or_log(format_args!("interface {number} -> alt {alt}"));
        self.usb.set_alt_setting(number, alt).map_err(Some)
    }

    /// Accept submissions again after an abort, so the engine can start on the same bus.
    pub fn resume(&mut self) {
        self.draining = false;
        self.timing = Timing::default();
    }

    /// Handle libusb events for up to `timeout`, and hand the transfers that came back to
    /// `engine`, through `audio` when there is one. An event-handling error fails the bus.
    pub fn poll(
        &mut self,
        engine: &mut Engine,
        timeout: Duration,
        audio: Option<&mut dyn Audio>,
    ) -> Result<(), UsbError> {
        let mut done = std::mem::take(&mut self.completions);
        let before = self.usb.now();
        let ret = self.usb.handle_events(timeout, &mut done);
        let after = self.usb.now();
        if let Err(error) = ret
            && error != UsbError::INTERRUPTED
        {
            self.failed = true;
        }
        self.time(before, after, &done);
        self.deliver(engine, &mut done, audio);
        self.completions = done;
        ret
    }

    /// Account one event wait from `before` to `after` that returned `done`, and log each
    /// direction whose completed PCM transfers were more than [`GAP_NS`] apart.
    fn time(&mut self, before: u64, after: u64, done: &[Completion]) {
        let t = &mut self.timing;
        if t.returned != 0 {
            t.outside = t.outside.max(before.saturating_sub(t.returned));
        }
        t.wait = t.wait.max(after.saturating_sub(before));
        t.returned = after;
        let mut any = false;
        let mut gaps = Vec::new();
        for c in done {
            if c.status != TRANSFER_COMPLETED {
                continue;
            }
            let endpoint = c.transfer.endpoint();
            let direction = if endpoint == Direction::Out.endpoint() {
                Direction::Out
            } else if endpoint == Direction::In.endpoint() {
                Direction::In
            } else {
                continue;
            };
            let d = dir_index(direction);
            let gap = after.saturating_sub(t.last[d]);
            if t.last[d] != 0 && gap > GAP_NS && !gaps.iter().any(|&(e, _)| e == d) {
                gaps.push((d, gap));
            }
            t.last[d] = after;
            any = true;
        }
        let (wait, outside) = (t.wait, t.outside);
        if any {
            t.wait = 0;
            t.outside = 0;
        }
        for (d, gap) in gaps {
            self.log(format_args!(
                "USB {} transfers paused {:.1} ms; longest wait in libusb {:.1} ms, longest time outside it {:.1} ms",
                if d == 0 { "out" } else { "in" },
                gap as f64 / 1e6,
                wait as f64 / 1e6,
                outside as f64 / 1e6
            ));
        }
    }

    /// Hand `done` to `engine`, one transfer at a time, leaving `done` empty.
    pub fn deliver(
        &mut self,
        engine: &mut Engine,
        done: &mut Vec<Completion>,
        mut audio: Option<&mut dyn Audio>,
    ) {
        for completion in done.drain(..) {
            let status = self.finish(&completion);
            let endpoint = completion.transfer.endpoint();
            if let (Status::Completed(_), Some(audio)) = (status, audio.as_deref_mut())
                && endpoint == Direction::Out.endpoint()
                && let Some(ring) = engine.pcm_ring_mut(Direction::Out)
            {
                audio.before_out(ring, self.usb.now());
            }
            engine.complete(self, completion.transfer, status);
            if std::mem::take(&mut self.input_ready)
                && let (Some(audio), Some(ring)) =
                    (audio.as_deref_mut(), engine.pcm_ring(Direction::In))
            {
                audio.after_in(ring);
            }
        }
    }

    /// A transfer back from libusb: count it, and how it reaches the engine. A transfer that
    /// failed fails the bus; a short input transfer passes on with its byte count, and a
    /// short output transfer fails the stream. While the bus drains or has failed, every
    /// transfer is aborted.
    pub fn finish(&mut self, c: &Completion) -> Status {
        self.pending -= 1;
        let endpoint = c.transfer.endpoint();
        let length = c.transfer.len();
        let mut aborted = false;
        if c.status != TRANSFER_COMPLETED {
            aborted = true;
            if !self.draining {
                self.log(format_args!(
                    "USB endpoint {endpoint:02x} failed: transfer status {}",
                    c.status
                ));
                self.failed = true;
            }
        } else if !self.draining {
            let dir = if endpoint & 0x80 != 0 { 1 } else { 0 };
            self.completed[dir] += 1;
            self.bytes[dir] += c.actual_length as u64;
            if c.actual_length != length {
                if endpoint == Direction::In.endpoint() {
                    if self.short_in == 0 {
                        self.log(format_args!(
                            "short PCM transfer on {endpoint:02x}: {}/{length}, missing frames silenced",
                            c.actual_length
                        ));
                    }
                    self.short_in += 1;
                } else if endpoint == Direction::Out.endpoint() {
                    self.log(format_args!(
                        "short PCM transfer on {endpoint:02x}: {}/{length}",
                        c.actual_length
                    ));
                    self.failed = true;
                    aborted = true;
                }
            }
        }
        let now = self.usb.now();
        self.trace_only(format_args!(
            "{now} complete ep={endpoint:02x} status={} bytes={}/{length}",
            c.status, c.actual_length
        ));
        if self.draining || self.failed || aborted {
            Status::Aborted
        } else {
            Status::Completed(c.actual_length)
        }
    }

    /// A control request, logged with its answer. After a request times out or finds no
    /// device, every request fails without reaching the device: an A16 that stopped answering
    /// stays that way until a port reset.
    fn control(&mut self, setup: ControlSetup, data: &mut [u8]) -> Result<(), BusError> {
        if setup.request_type & 0x80 != 0 {
            data.fill(0);
        }
        if self.failed {
            return Err(BusError(UsbError::IO.0));
        }
        let ret = self.usb.control(
            setup.request_type,
            setup.request,
            setup.value,
            setup.index,
            data,
            CONTROL_TIMEOUT,
        );
        if matches!(ret, Err(UsbError::TIMEOUT) | Err(UsbError::NO_DEVICE)) {
            self.failed = true;
        }
        let (result, got) = match ret {
            Ok(n) => (n as i64, n),
            Err(error) => (i64::from(error.0), 0),
        };
        let mut hex = String::new();
        for byte in &data[..got.min(data.len())] {
            hex.push_str(&format!("{byte:02x}"));
        }
        let now = self.usb.now();
        self.trace_or_log(format_args!(
            "{now} control type={:02x} req={:02x} value={:04x} index={:04x} length={} result={result} data={hex}",
            setup.request_type,
            setup.request,
            setup.value,
            setup.index,
            data.len()
        ));
        let size = data.len() as i64;
        // 'V' is variable length: the engine asks for 15 bytes and parses only the
        // three-byte prefix, which is all the A16 returns
        let firmware = ControlSetup {
            request_type: 0xc0,
            request: 0x56,
            value: 0,
            index: 0,
        };
        let complete = if setup == firmware && size == 15 {
            (3..=size).contains(&result)
        } else {
            result == size
        };
        match (complete, ret) {
            (true, _) => Ok(()),
            (false, Err(error)) => Err(BusError(error.0)),
            (false, Ok(_)) => Err(BusError(UsbError::IO.0)),
        }
    }

    /// Release the interfaces, after cancelling what is in flight, and give interfaces
    /// back to the kernel drivers detached from them. The interfaces stay at their streaming
    /// alternate setting: releasing them sends the A16 nothing, and a port reset or the next
    /// open sets them again.
    pub fn release(&mut self) {
        drop(self.abort());
        for i in (0..=255u8).rev() {
            let at = usize::from(i);
            if self.claimed[at]
                && let Err(error) = self.usb.release_interface(i)
                && error != UsbError::NO_DEVICE
            {
                self.log(format_args!("release interface {i}: {error}"));
                self.failed = true;
            }
            if self.detached[at]
                && let Err(error) = self.usb.attach_kernel_driver(i)
            {
                self.log(format_args!("reattach interface {i}: {error}"));
                self.failed = true;
            }
            self.claimed[at] = false;
            self.detached[at] = false;
        }
    }
}

/// What moves audio between the rings and the platform's queues, on the completion path.
pub trait Audio {
    /// Before an out transfer that completed at `now` encodes the next one from `ring`.
    fn before_out(&mut self, ring: &mut PcmRing, now: u64);

    /// After an in transfer decoded its frames into `ring`.
    fn after_in(&mut self, ring: &PcmRing);
}

impl<U: Usb> Bus for UsbBus<U> {
    fn submit(&mut self, transfer: Transfer) -> Result<(), Transfer> {
        let endpoint = transfer.endpoint();
        let kind = match self.types[usize::from(endpoint)] {
            LIBUSB_TRANSFER_TYPE_BULK => Some(TransferKind::Bulk),
            LIBUSB_TRANSFER_TYPE_INTERRUPT => Some(TransferKind::Interrupt),
            _ => None,
        };
        let size = transfer.len();
        let Some(kind) = kind
            .filter(|_| !self.draining && !self.failed && size != 0 && size <= i32::MAX as usize)
        else {
            self.failed = true;
            return Err(transfer);
        };
        match self.usb.submit(kind, transfer, TRANSFER_TIMEOUT) {
            Ok(()) => {
                self.pending += 1;
                let now = self.usb.now();
                self.trace_only(format_args!("{now} submit ep={endpoint:02x} bytes={size}"));
                Ok(())
            }
            Err((error, transfer)) => {
                self.log(format_args!("submit {endpoint:02x}: {error}"));
                self.failed = true;
                Err(transfer)
            }
        }
    }

    fn control_in(&mut self, setup: ControlSetup, data: &mut [u8]) -> Result<(), BusError> {
        self.control(setup, data)
    }

    fn control_out(&mut self, setup: ControlSetup, data: &[u8]) -> Result<(), BusError> {
        let mut data = data.to_vec();
        self.control(setup, &mut data)
    }

    fn clear_halt(&mut self, endpoint: u8) -> Result<(), BusError> {
        let ret = self.usb.clear_halt(endpoint);
        let now = self.usb.now();
        let code = ret.err().map_or(0, |e| e.0);
        self.trace_or_log(format_args!(
            "{now} clear-halt ep={endpoint:02x} result={code}"
        ));
        ret.map_err(|e| BusError(e.0))
    }

    fn sleep(&mut self, duration: Duration) {
        self.usb.sleep(duration);
    }

    /// Cancel every transfer in flight and wait, up to 5 s, until libusb gives each back. A
    /// transfer libusb still holds past that may still use its buffer, so the process exits
    /// then.
    fn abort(&mut self) -> Vec<Transfer> {
        self.draining = true;
        for error in self.usb.cancel_all() {
            if error != UsbError::NOT_FOUND {
                self.log(format_args!("cancel: {error}"));
            }
        }
        let deadline = self.usb.now() + DRAIN_LIMIT_NS;
        let mut aborted = Vec::new();
        while self.pending != 0 {
            let mut done = Vec::new();
            let ret = self.usb.handle_events(Duration::from_millis(10), &mut done);
            for completion in done {
                self.finish(&completion);
                aborted.push(completion.transfer);
            }
            let failed = ret.is_err_and(|error| error != UsbError::INTERRUPTED);
            if failed || self.usb.now() > deadline {
                self.log(format_args!(
                    "USB cancellation did not drain; terminating with buffers intact"
                ));
                std::process::exit(1);
            }
        }
        aborted
    }

    fn pcm_progress(&mut self, direction: Direction, position: u64, _ring: &mut PcmRing) {
        self.progress[dir_index(direction)] = position;
        if direction == Direction::In && !self.draining && !self.failed {
            self.input_ready = true;
        }
    }
}
