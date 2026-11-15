// SPDX-License-Identifier: MIT
//! A fake A16 behind the [`Usb`] trait, on a fake clock: it streams at the A16's pace,
//! leaves and rejoins the bus, stops answering control requests, and records every libusb
//! call and every line the driver logs, with the time.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use a16_engine::{Engine, Transfer};
use open_realiser_a16_driver::bridge::{AudioPath, Bridge, Shared};
use open_realiser_a16_driver::bus::{Log, UsbBus, engine_logger};
use open_realiser_a16_driver::profiles::{self, Missing};
use open_realiser_a16_driver::usb::{
    AltSetting, Completion, ConfigDescriptor, EndpointDescriptor, TRANSFER_CANCELLED,
    TRANSFER_COMPLETED, TRANSFER_NO_DEVICE, TransferKind, Usb, UsbError,
};

pub const MS: u64 = 1_000_000;
pub const S: u64 = 1_000_000_000;
/// One 80-frame transfer at 48 kHz.
const PERIOD: u64 = 80 * S / 48000;

/// Something that happens to the A16, or around it, at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The A16 drops off the bus: transfers in flight fail, the open handle goes stale.
    Leave,
    /// The A16 enumerates again, as a new device.
    Arrive,
    /// The A16 stops answering control requests until a port reset.
    StopAnswering,
    /// The next transfer to complete fails with this `libusb_transfer_status`; the A16
    /// stays on the bus.
    FailTransfer(i32),
    /// The return starts (true) or stops carrying audio.
    Loud(bool),
    /// A client starts or stops playing.
    Playback(bool),
    /// A client starts or stops recording.
    Recording(bool),
    /// SIGTERM.
    Stop,
}

pub struct State {
    pub now: u64,
    /// The A16 is on the bus.
    pub on_bus: bool,
    /// Counts enumerations: a handle is stale once the A16 enumerated again.
    pub generation: u32,
    open: Option<u32>,
    gone: bool,
    /// Control requests time out until a port reset.
    pub unanswering: bool,
    /// How long a port reset blocks.
    pub reset_ns: u64,
    /// What the next resets return, before they succeed.
    pub reset_errors: VecDeque<UsbError>,
    pub loud: bool,
    fail_next: Option<i32>,
    schedule: VecDeque<(u64, Event)>,
    in_flight: VecDeque<(Transfer, u64)>,
    done: VecDeque<(Transfer, i32, usize)>,
    next_due: [u64; 2],
    rate: [u8; 3],
    i_reg: u8,
    shared: Option<Arc<Shared>>,
    stopping: Arc<AtomicBool>,
    /// Every libusb call the driver made, and every line it logged, with the time.
    pub log: Vec<(u64, String)>,
}

impl State {
    fn note(&mut self, line: String) {
        self.log.push((self.now, line));
    }

    fn stale(&self) -> bool {
        !self.on_bus || self.open != Some(self.generation)
    }

    fn apply(&mut self, event: Event) {
        self.note(format!("event {event:?}"));
        match event {
            Event::Leave => {
                self.on_bus = false;
                for (transfer, _) in std::mem::take(&mut self.in_flight) {
                    self.done.push_back((transfer, TRANSFER_NO_DEVICE, 0));
                }
                if self.open == Some(self.generation) {
                    self.gone = true;
                }
            }
            Event::Arrive => {
                self.on_bus = true;
                self.generation += 1;
                self.rate = [0x44, 0xac, 0x00];
            }
            Event::StopAnswering => self.unanswering = true,
            Event::FailTransfer(status) => self.fail_next = Some(status),
            Event::Loud(loud) => self.loud = loud,
            Event::Playback(on) => {
                if let Some(shared) = &self.shared {
                    shared.sink.active.store(on, Ordering::SeqCst);
                }
            }
            Event::Recording(on) => {
                if let Some(shared) = &self.shared {
                    shared.source.active.store(on, Ordering::SeqCst);
                }
            }
            Event::Stop => self.stopping.store(true, Ordering::SeqCst),
        }
    }

    /// Move the clock to `to`, applying the events on the way.
    fn advance(&mut self, to: u64) {
        while let Some(&(at, event)) = self.schedule.front() {
            if at > to {
                break;
            }
            self.schedule.pop_front();
            self.now = self.now.max(at);
            self.apply(event);
        }
        self.now = self.now.max(to);
    }

    /// Complete the transfers due by now, in order.
    fn complete_due(&mut self) {
        while let Some((_, due)) = self.in_flight.front() {
            if *due > self.now {
                break;
            }
            let (mut transfer, _) = self.in_flight.pop_front().expect("front");
            let status = self.fail_next.take().unwrap_or(TRANSFER_COMPLETED);
            let len = transfer.len();
            if transfer.endpoint() & 0x80 != 0 {
                fill_return(&mut transfer, self.loud);
            }
            let actual = if status == TRANSFER_COMPLETED { len } else { 0 };
            self.done.push_back((transfer, status, actual));
        }
    }
}

/// An in transfer of a silent return, or of one at -24 dBFS on both channels.
fn fill_return(transfer: &mut Transfer, loud: bool) {
    let buffer = transfer.buffer_mut();
    buffer.fill(0);
    if !loud {
        return;
    }
    for frame in buffer.as_chunks_mut::<64>().0 {
        for bit in 0..24 {
            let b = ((0x08_0000u32 >> (23 - bit)) & 1) as u8;
            frame[bit] = b;
            frame[32 + bit] = b;
        }
    }
}

/// The fake: a handle on the shared state.
#[derive(Clone)]
pub struct FakeA16(pub Arc<Mutex<State>>);

impl FakeA16 {
    /// An A16 on the bus at t = 1 s, answering, silent, and `events` to come.
    pub fn new(events: &[(u64, Event)]) -> FakeA16 {
        let mut schedule: Vec<(u64, Event)> = events.to_vec();
        schedule.sort_by_key(|&(at, _)| at);
        FakeA16(Arc::new(Mutex::new(State {
            now: S,
            on_bus: true,
            generation: 0,
            open: None,
            gone: false,
            unanswering: false,
            reset_ns: 150 * MS,
            reset_errors: VecDeque::new(),
            loud: false,
            fail_next: None,
            schedule: schedule.into(),
            in_flight: VecDeque::new(),
            done: VecDeque::new(),
            next_due: [0; 2],
            rate: [0x44, 0xac, 0x00],
            i_reg: 0,
            shared: None,
            stopping: Arc::new(AtomicBool::new(false)),
            log: Vec::new(),
        })))
    }

    pub fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().expect("fake state")
    }

    /// The time of the first log line that starts with `prefix`, at or after `from`.
    pub fn first(&self, prefix: &str, from: u64) -> Option<u64> {
        self.state()
            .log
            .iter()
            .find(|(t, line)| *t >= from && line.starts_with(prefix))
            .map(|&(t, _)| t)
    }

    /// The lines that start with `prefix`.
    pub fn lines(&self, prefix: &str) -> Vec<(u64, String)> {
        self.state()
            .log
            .iter()
            .filter(|(_, line)| line.starts_with(prefix))
            .cloned()
            .collect()
    }

    /// Every line, for a test that failed.
    pub fn dump(&self) -> String {
        self.state()
            .log
            .iter()
            .filter(|(_, l)| !l.starts_with("usb submit"))
            .map(|(t, l)| format!("{:>9.3} {l}\n", *t as f64 / 1e9))
            .collect()
    }

    fn call(&self, line: impl FnOnce() -> String) -> std::sync::MutexGuard<'_, State> {
        let mut s = self.state();
        let line = line();
        s.note(format!("usb {line}"));
        s
    }
}

fn a16_config() -> ConfigDescriptor {
    let setting = |interface, alt_setting, endpoints| AltSetting {
        interface,
        alt_setting,
        class: 0xff,
        subclass: 0,
        protocol: 0,
        endpoints,
    };
    let ep = |address, max_packet_size| EndpointDescriptor {
        address,
        attributes: 3,
        max_packet_size,
        interval: 1,
    };
    ConfigDescriptor {
        value: 1,
        interfaces: 2,
        settings: vec![
            setting(0, 0, vec![]),
            setting(0, 1, vec![ep(0x05, 964), ep(0x83, 512)]),
            setting(1, 0, vec![]),
            setting(1, 1, vec![ep(0x86, 1024)]),
        ],
    }
}

impl Usb for FakeA16 {
    fn now(&self) -> u64 {
        self.state().now
    }

    fn sleep(&mut self, duration: Duration) {
        let mut s = self.call(|| format!("sleep {}", duration.as_millis()));
        let to = s.now + duration.as_nanos() as u64;
        s.advance(to);
    }

    fn find_a16(&mut self) -> bool {
        self.state().on_bus
    }

    fn open_a16(&mut self) -> Result<(), UsbError> {
        let mut s = self.call(|| "open".to_owned());
        if !s.on_bus {
            return Err(UsbError::NOT_FOUND);
        }
        s.open = Some(s.generation);
        Ok(())
    }

    fn close(&mut self) {
        let mut s = self.state();
        if s.open.take().is_some() {
            s.note("usb close".to_owned());
        }
    }

    fn reset(&mut self) -> Result<(), UsbError> {
        let mut s = self.call(|| "reset".to_owned());
        if s.stale() {
            return Err(UsbError::NOT_FOUND);
        }
        let to = s.now + s.reset_ns;
        s.advance(to);
        if let Some(error) = s.reset_errors.pop_front() {
            return Err(error);
        }
        if s.stale() {
            return Err(UsbError::NOT_FOUND);
        }
        s.unanswering = false;
        Ok(())
    }

    fn gone(&self) -> bool {
        self.state().gone
    }

    fn clear_gone(&mut self) {
        self.state().gone = false;
    }

    fn active_config(&mut self) -> Result<ConfigDescriptor, UsbError> {
        let s = self.call(|| "config".to_owned());
        if s.stale() {
            return Err(UsbError::NO_DEVICE);
        }
        Ok(a16_config())
    }

    fn kernel_driver_active(&mut self, _interface: u8) -> Result<bool, UsbError> {
        Ok(false)
    }

    fn detach_kernel_driver(&mut self, _interface: u8) -> Result<(), UsbError> {
        Ok(())
    }

    fn attach_kernel_driver(&mut self, _interface: u8) -> Result<(), UsbError> {
        Ok(())
    }

    fn claim_interface(&mut self, interface: u8) -> Result<(), UsbError> {
        let s = self.call(|| format!("claim {interface}"));
        if s.stale() {
            Err(UsbError::NO_DEVICE)
        } else {
            Ok(())
        }
    }

    fn release_interface(&mut self, interface: u8) -> Result<(), UsbError> {
        let s = self.call(|| format!("release {interface}"));
        if s.stale() {
            Err(UsbError::NO_DEVICE)
        } else {
            Ok(())
        }
    }

    fn set_alt_setting(&mut self, interface: u8, alt_setting: u8) -> Result<(), UsbError> {
        let s = self.call(|| format!("alt {interface} {alt_setting}"));
        if s.stale() {
            Err(UsbError::NO_DEVICE)
        } else {
            Ok(())
        }
    }

    fn control(
        &mut self,
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        data: &mut [u8],
        timeout: Duration,
    ) -> Result<usize, UsbError> {
        let mut s = self
            .call(|| format!("control {request_type:02x} {request:02x} {value:04x} {index:04x}"));
        if s.stale() {
            return Err(UsbError::NO_DEVICE);
        }
        if s.unanswering {
            let to = s.now + timeout.as_nanos() as u64;
            s.advance(to);
            return Err(UsbError::TIMEOUT);
        }
        let to = s.now + 100_000;
        s.advance(to);
        match (request_type, request) {
            (0xc0, 0x56) => {
                data[..3].copy_from_slice(&[1, 2, 3]);
                Ok(3)
            }
            (0xc0, 0x49) => {
                data[0] = s.i_reg;
                Ok(1)
            }
            (0x40, 0x49) => {
                s.i_reg = (value & 0xff) as u8;
                Ok(0)
            }
            (0xa2, 0x81) => {
                data[..3].copy_from_slice(&s.rate);
                Ok(3)
            }
            (0x22, 0x01) => {
                s.rate.copy_from_slice(&data[..3]);
                Ok(3)
            }
            _ => Ok(data.len()),
        }
    }

    fn clear_halt(&mut self, endpoint: u8) -> Result<(), UsbError> {
        let s = self.call(|| format!("clear-halt {endpoint:02x}"));
        if s.stale() {
            Err(UsbError::NO_DEVICE)
        } else {
            Ok(())
        }
    }

    fn submit(
        &mut self,
        _kind: TransferKind,
        transfer: Transfer,
        _timeout: Duration,
    ) -> Result<(), (UsbError, Transfer)> {
        let mut s = self.call(|| format!("submit {:02x}", transfer.endpoint()));
        if s.stale() {
            return Err((UsbError::NO_DEVICE, transfer));
        }
        let dir = usize::from(transfer.endpoint() & 0x80 != 0);
        let due = s.now.max(s.next_due[dir]) + PERIOD;
        s.next_due[dir] = due;
        // Each endpoint completes in order; the queue keeps both, by due time
        let at = s
            .in_flight
            .iter()
            .position(|(_, d)| *d > due)
            .unwrap_or(s.in_flight.len());
        s.in_flight.insert(at, (transfer, due));
        Ok(())
    }

    fn cancel_all(&mut self) -> Vec<UsbError> {
        let mut s = self.call(|| "cancel".to_owned());
        for (transfer, _) in std::mem::take(&mut s.in_flight) {
            s.done.push_back((transfer, TRANSFER_CANCELLED, 0));
        }
        s.next_due = [0; 2];
        Vec::new()
    }

    fn handle_events(
        &mut self,
        timeout: Duration,
        done: &mut Vec<Completion>,
    ) -> Result<(), UsbError> {
        let mut s = self.state();
        if s.done.is_empty() {
            let limit = s.now + timeout.as_nanos() as u64;
            let due = s.in_flight.front().map_or(u64::MAX, |&(_, d)| d);
            s.advance(due.min(limit));
            s.complete_due();
        }
        for (transfer, status, actual_length) in s.done.drain(..) {
            done.push(Completion {
                transfer,
                status,
                actual_length,
            });
        }
        Ok(())
    }
}

/// A logger into the fake's log, with the fake's time.
pub fn log_into(fake: &FakeA16, prefix: &'static str) -> Log {
    let fake = fake.clone();
    Box::new(move |line| fake.state().note(format!("{prefix}{line}")))
}

/// The bridge on `fake`, with the default profile, and the flag SIGTERM sets.
pub fn bridge(fake: &FakeA16) -> (Bridge<FakeA16>, Arc<AtomicBool>) {
    let effective = profiles::effective(&Missing::default()).expect("profiles");
    let shared = Arc::new(Shared::new(effective, 0));
    let stopping = {
        let mut s = fake.state();
        s.shared = Some(Arc::clone(&shared));
        Arc::clone(&s.stopping)
    };
    let mut engine = Engine::new();
    let sink = fake.clone();
    engine.set_logger(engine_logger(move |line| {
        sink.state().note(format!("engine {line}"))
    }));
    let bus = UsbBus::new(fake.clone(), None, log_into(fake, "bus "));
    let audio = AudioPath::new(Arc::clone(&shared), log_into(fake, "log ")).expect("resamplers");
    let bridge = Bridge::new(bus, engine, shared, audio, false, log_into(fake, "log "));
    (bridge, stopping)
}

/// Run the driver's USB lifecycle on `fake` until it ends, at most `limit` of fake time
/// from now (a SIGTERM then): its exit status.
pub fn drive(fake: &FakeA16, limit: u64) -> i32 {
    let (mut bridge, stopping) = bridge(fake);
    {
        let mut s = fake.state();
        let at = s.now + limit;
        let position = s
            .schedule
            .iter()
            .position(|&(t, _)| t > at)
            .unwrap_or(s.schedule.len());
        s.schedule.insert(position, (at, Event::Stop));
    }
    let status = bridge.drive(&stopping, 0, || {});
    fake.state().note(format!("exit {status}"));
    status
}
