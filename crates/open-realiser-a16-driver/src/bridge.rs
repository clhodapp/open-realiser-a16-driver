// SPDX-License-Identifier: MIT
//! The USB side of the PipeWire bridge: when the A16 streams, how audio moves between the
//! PipeWire queues and the engine's rings, and how the bridge recovers when the A16 drops
//! off USB. It runs on one thread against [`Usb`], so the tests drive it with a fake A16 and
//! a fake clock.
//!
//! The A16 drops off USB when it receives playback about 43.5 s after a power-up enumerates
//! it, sooner when the cable is connected partway through a boot, and again 0.5 to 1.5 s
//! after playback resumes, until about 50 s into the boot
//! (`docs/development/boot-usb-drop.md`). A failure counts as such a drop while a client is
//! active, before the A16 has returned a sustained audio level ([`Level`]), and inside the
//! boot window: [`BOOT_FIRST_NS`] from the first open, then after each boot-time drop until
//! [`BOOT_EXTEND_NS`] after the stream may resume.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use a16_engine::{Direction, Engine, PcmRing, SampleRate};

use crate::bus::{Audio, Log, UsbBus};
use crate::graph_clock::{A16Clock, Earliest};
use crate::live::{self, Clock, Fifo, Level};
use crate::profiles::Effective;
use crate::samplerate::Resampler;
use crate::usb::{Usb, UsbError};

/// The rate the bridge streams at.
pub const RATE: u32 = 48000;
/// Frames per PCM transfer, each way.
pub const BLOCK: usize = 80;
/// How long the A16 may stay off the bus after its boot-time drop before the driver exits.
/// The A16 re-enumerates within a second after that drop.
pub const LOST_WAIT_NS: u64 = 3_000_000_000;
/// The boot window from the first open.
pub const BOOT_FIRST_NS: u64 = 50_000_000_000;
/// The boot window after a boot-time drop, from when the stream may resume.
pub const BOOT_EXTEND_NS: u64 = 5_000_000_000;
/// After a USB failure, no request goes to the reopened A16 before this long after the
/// failure, or before its port reset finishes if that is later. After a boot-time drop the
/// A16 answered requests sent 6 to 8 s later, and stopped answering some sent 4 s later.
pub const RECOVERY_QUIET_NS: u64 = 9_000_000_000;
/// Port resets tried on each reopen after a USB failure.
pub const RESET_TRIES: u32 = 3;
/// Reopens tried on an A16 that is on the bus, after a USB failure.
pub const REOPEN_TRIES: u32 = 3;
/// More failures than this within [`RECOVERY_WINDOW_NS`] end the driver.
pub const RECOVERY_LIMIT: usize = 5;
pub const RECOVERY_WINDOW_NS: u64 = 120_000_000_000;
/// How often the bridge logs its stats while streaming.
const STATS_NS: u64 = 5_000_000_000;

/// One direction between PipeWire and USB.
pub struct Side {
    pub fifo: Fifo,
    /// The node is streaming.
    pub active: AtomicBool,
    /// The queue's consumer discards what is queued and primes again.
    pub reset: AtomicBool,
    pub frames: AtomicU64,
    pub underruns: AtomicU64,
    pub overruns: AtomicU64,
}

impl Side {
    fn new(channels: usize) -> Side {
        Side {
            fifo: Fifo::new(channels),
            active: AtomicBool::new(false),
            reset: AtomicBool::new(false),
            frames: AtomicU64::new(0),
            underruns: AtomicU64::new(0),
            overruns: AtomicU64::new(0),
        }
    }
}

/// What the PipeWire threads and the USB thread share.
pub struct Shared {
    /// Playback: frames expanded to the 16 USB slots.
    pub sink: Side,
    /// The stereo return.
    pub source: Side,
    /// A PipeWire or audio error: the bridge ends.
    pub failed: AtomicBool,
    output_ratio: AtomicU64,
    capture_ratio: AtomicU64,
    /// The frames the A16 has consumed and when, which clock the PipeWire graph.
    pub a16_clock: A16Clock,
    /// The sink's last cycle: its `rate_diff`, and whether it followed the A16 clock.
    pub graph_rate_diff: AtomicU64,
    pub graph_locked: AtomicBool,
    /// The selected profile, an index of `effective`.
    pub profile: AtomicUsize,
    pub effective: Vec<Effective>,
}

impl Shared {
    pub fn new(effective: Vec<Effective>, profile: usize) -> Shared {
        Shared {
            sink: Side::new(live::CHANNELS),
            source: Side::new(2),
            failed: AtomicBool::new(false),
            output_ratio: AtomicU64::new(0),
            capture_ratio: AtomicU64::new(0),
            a16_clock: A16Clock::default(),
            graph_rate_diff: AtomicU64::new(1f64.to_bits()),
            graph_locked: AtomicBool::new(false),
            profile: AtomicUsize::new(profile),
            effective,
        }
    }

    /// The profile the sink runs.
    pub fn profile(&self) -> &Effective {
        &self.effective[self.profile.load(Ordering::Relaxed)]
    }
}

/// The audio path on the USB thread: resampling between the queues and the 80-frame rings,
/// each direction steered by the level of its queue, and the level check on the return.
pub struct AudioPath {
    shared: Arc<Shared>,
    out_src: Resampler,
    in_src: Resampler,
    out_clock: Clock,
    in_clock: Clock,
    output_primed: bool,
    /// The A16 has returned a sustained audio level, so it has finished booting.
    pub booted: bool,
    return_level: Level,
    /// The frames the A16 has taken in out transfers since the bridge started.
    out_frames: u64,
    earliest: Earliest,
    log: Log,
}

impl AudioPath {
    /// None when libsamplerate makes no converter.
    pub fn new(shared: Arc<Shared>, log: Log) -> Option<AudioPath> {
        Some(AudioPath {
            shared,
            out_src: Resampler::new(live::CHANNELS)?,
            in_src: Resampler::new(2)?,
            out_clock: Clock::default(),
            in_clock: Clock::default(),
            output_primed: false,
            booted: false,
            return_level: Level::default(),
            out_frames: 0,
            earliest: Earliest::default(),
            log,
        })
    }

    /// Date the A16's frames afresh: the stream starts again.
    pub fn restart_clock(&mut self) {
        self.earliest.reset();
    }

    fn reset_output(&mut self) {
        self.shared.sink.fifo.discard();
        self.output_primed = false;
        self.out_clock = Clock::default();
        self.out_src.reset();
    }
}

const BLOCK_SECONDS: f64 = BLOCK as f64 / RATE as f64;

impl Audio for AudioPath {
    /// The next out transfer's 80 frames, from the playback queue: silence until the queue
    /// has filled to its target, and after it runs dry until it fills again. While the
    /// playback node is paused the queued audio keeps playing out, so a brief pause of the
    /// node, as PipeWire makes when it reconfigures the graph, is not heard. The transfer
    /// that completed at `now` carried 80 frames the A16 has taken, which advances the
    /// graph's clock.
    fn before_out(&mut self, ring: &mut PcmRing, now: u64) {
        self.out_frames += BLOCK as u64;
        let sample = self.earliest.date(self.out_frames, now);
        self.shared.a16_clock.publish(sample);
        let out = ring.as_bytes_mut();
        out.fill(0);
        let shared = Arc::clone(&self.shared);
        let sink = &shared.sink;
        if sink.reset.swap(false, Ordering::SeqCst) {
            self.reset_output();
        }
        let active = sink.active.load(Ordering::SeqCst);
        let available = sink.fifo.available();
        if !self.output_primed {
            if !active || available < live::TARGET {
                return;
            }
            self.output_primed = true;
        }
        let mut input = [0.0f32; 256 * live::CHANNELS];
        let mut output = [0.0f32; BLOCK * live::CHANNELS];
        let n = sink.fifo.peek(&mut input) as usize;
        let ratio = self.out_clock.ratio(available, BLOCK_SECONDS);
        shared.output_ratio.store(ratio.to_bits(), Ordering::SeqCst);
        let Ok(block) = self
            .out_src
            .process(&input[..n * live::CHANNELS], &mut output, ratio)
        else {
            shared.failed.store(true, Ordering::SeqCst);
            return;
        };
        sink.fifo.consume(block.input_used as u32);
        if block.output_generated < BLOCK {
            if active {
                sink.underruns.fetch_add(1, Ordering::SeqCst);
                self.output_primed = false;
                self.out_src.reset();
            } else {
                // The queue has played out while the node is paused: the stream has stopped
                self.reset_output();
            }
        }
        for (i, &v) in output.iter().enumerate() {
            live::pack(&mut out[i * 3..i * 3 + 3], v);
        }
    }

    /// An in transfer's 80 frames, into the capture queue while the return node streams.
    fn after_in(&mut self, ring: &PcmRing) {
        let bytes = ring.as_bytes();
        let mut input = [0.0f32; BLOCK * 2];
        for (i, v) in input.iter_mut().enumerate() {
            *v = live::unpack(&bytes[i * 3..i * 3 + 3]);
        }
        // The A16 returns silence while it boots, and processed audio once booted
        if !self.booted && self.return_level.feed(&input, 2) {
            self.booted = true;
            (self.log)("A16 return carries audio; the A16 has finished booting");
        }
        let shared = Arc::clone(&self.shared);
        let source = &shared.source;
        // Only the capture consumer owns its read index. While idle, do not produce.
        if !source.active.load(Ordering::SeqCst) {
            self.in_src.reset();
            self.in_clock = Clock::default();
            return;
        }
        let ratio = self.in_clock.ratio(source.fifo.available(), BLOCK_SECONDS);
        shared
            .capture_ratio
            .store(ratio.to_bits(), Ordering::SeqCst);
        let mut output = [0.0f32; 256 * 2];
        match self.in_src.process(&input, &mut output, ratio) {
            Ok(block) if block.input_used == BLOCK => {
                let n = block.output_generated;
                if (source.fifo.push(&output[..n * 2]) as usize) < n {
                    source.overruns.fetch_add(1, Ordering::SeqCst);
                }
            }
            _ => shared.failed.store(true, Ordering::SeqCst),
        }
    }
}

fn seconds(ns: u64) -> f64 {
    ns as f64 / 1e9
}

/// The A16 on USB, as the bridge drives it.
pub struct Bridge<U: Usb> {
    pub bus: UsbBus<U>,
    pub engine: Engine,
    pub shared: Arc<Shared>,
    pub audio: AudioPath,
    detach: bool,
    configured: bool,
    attached: bool,
    streaming: bool,
    /// End of the boot window.
    boot_until: u64,
    /// No USB requests before this time, after a failure.
    hold_until: u64,
    /// When the last USB failure was detected; 0 before any.
    last_failure: u64,
    log: Log,
}

impl<U: Usb> Bridge<U> {
    pub fn new(
        bus: UsbBus<U>,
        engine: Engine,
        shared: Arc<Shared>,
        audio: AudioPath,
        detach: bool,
        log: Log,
    ) -> Bridge<U> {
        Bridge {
            bus,
            engine,
            shared,
            audio,
            detach,
            configured: false,
            attached: false,
            streaming: false,
            boot_until: 0,
            hold_until: 0,
            last_failure: 0,
            log,
        }
    }

    fn log(&mut self, args: std::fmt::Arguments<'_>) {
        (self.log)(&args.to_string());
    }

    fn now(&self) -> u64 {
        self.bus.usb.now()
    }

    /// Stop the engine, release the interfaces and close the A16.
    pub fn close(&mut self) {
        if self.attached {
            self.engine.stop(&mut self.bus);
        }
        if self.configured {
            self.bus.release();
        }
        self.bus.usb.close();
        self.attached = false;
        self.configured = false;
        self.streaming = false;
    }

    /// Open and configure the A16 and attach the rings, without streaming. With `reset`,
    /// the A16's USB port is reset first: after the A16 drops off USB it answers nothing
    /// for several seconds, and the kernel's reset retries until it does.
    pub fn open(&mut self, reset: bool) -> bool {
        if self.bus.usb.open_a16().is_err() {
            return false;
        }
        let mut ret = Ok(());
        for _ in 0..if reset { RESET_TRIES } else { 0 } {
            let began = self.now();
            ret = self.bus.usb.reset();
            let took = seconds(self.now() - began);
            let what = match ret {
                Ok(()) => "done",
                Err(error) => error.name(),
            };
            self.log(format_args!("USB reset {what} after {took:.2} s"));
            match ret {
                Ok(()) => break,
                // The A16 left the bus again: the caller finds it anew
                Err(UsbError::NOT_FOUND | UsbError::NO_DEVICE) => break,
                Err(_) => {}
            }
        }
        if ret.is_err() {
            self.close();
            return false;
        }
        self.bus.reset_state();
        self.configured = true;
        if !self.bus.configure(&mut self.engine, self.detach) {
            self.close();
            return false;
        }
        self.engine.set_rate(Some(SampleRate::Hz48000));
        self.engine
            .attach_pcm_ring(PcmRing::new(Direction::Out, BLOCK), 0);
        self.engine
            .attach_pcm_ring(PcmRing::new(Direction::In, BLOCK), 0);
        self.attached = true;
        self.bus.usb.clear_gone();
        if !reset {
            self.boot_until = self.now() + BOOT_FIRST_NS;
        }
        true
    }

    /// Run the handshake and start streaming.
    fn stream(&mut self) -> bool {
        self.audio.restart_clock();
        self.shared.sink.reset.store(true, Ordering::SeqCst);
        self.shared.source.reset.store(true, Ordering::SeqCst);
        self.bus.resume();
        if self.engine.start(&mut self.bus).is_err() || self.bus.failed {
            return false;
        }
        if self.engine.sample_rate() != Some(SampleRate::Hz48000)
            || self.engine.transfer_frames(Direction::Out) as usize != BLOCK
            || self.engine.transfer_frames(Direction::In) as usize != BLOCK
        {
            self.log(format_args!("Unexpected USB rate or transfer size"));
            return false;
        }
        self.streaming = true;
        true
    }

    /// After a USB failure: close the A16 and reopen it with a port reset. With
    /// `boot_drop`, the A16 may be off the bus for up to [`LOST_WAIT_NS`] first; otherwise
    /// an A16 off the bus ends recovery at once. A single reset can take several seconds
    /// while the kernel retries, and the A16 stays on the bus throughout, so
    /// [`REOPEN_TRIES`] bounds the reopens of an A16 that is present.
    fn recover(&mut self, boot_drop: bool, stopping: &AtomicBool) -> Recovery {
        self.close();
        let mut absent_since = self.now();
        let mut reopens = 0;
        while !stopping.load(Ordering::SeqCst) {
            if self.bus.usb.find_a16() {
                if self.open(true) {
                    return Recovery::Open;
                }
                reopens += 1;
                if reopens >= REOPEN_TRIES {
                    return Recovery::Lost;
                }
                absent_since = self.now();
            } else if !boot_drop || self.now() - absent_since >= LOST_WAIT_NS {
                return Recovery::Lost;
            }
            self.bus.usb.sleep(Duration::from_millis(100));
        }
        Recovery::Stopping
    }

    fn wanted(&self) -> bool {
        self.shared.sink.active.load(Ordering::SeqCst)
            || self.shared.source.active.load(Ordering::SeqCst)
    }

    /// Stream on demand and recover from USB failures until `stopping`, a PipeWire failure,
    /// `seconds_bound` seconds (0: no bound), an A16 that left the bus, or too many
    /// failures: the exit status. The A16 is open.
    pub fn run(&mut self, stopping: &AtomicBool, seconds_bound: u64) -> i32 {
        let start = self.now();
        let mut last = start;
        let mut failures: Vec<u64> = Vec::with_capacity(RECOVERY_LIMIT);
        // Set once streaming has started: from then on the A16 streams until the driver
        // exits, with silence while no client plays, so a client starting later loses no
        // audio
        let mut keep_streaming = false;
        let mut result = 0;
        self.log(format_args!(
            "READY: A16 attached, 16ch out/2ch in, 48000 Hz; USB streaming starts when a client first plays or records"
        ));
        // The PipeWire nodes exist and the A16 is open: services ordered after the driver
        // can start
        crate::notify::ready();
        while !stopping.load(Ordering::SeqCst)
            && !self.shared.failed.load(Ordering::SeqCst)
            && (seconds_bound == 0 || self.now() - start < seconds_bound * 1_000_000_000)
        {
            let wanted = keep_streaming || self.wanted();
            let mut broken = false;
            if self.streaming {
                let _ = self.bus.poll(
                    &mut self.engine,
                    Duration::from_millis(1),
                    Some(&mut self.audio),
                );
                if self.bus.failed || self.bus.usb.gone() {
                    broken = true;
                }
            } else if wanted && self.now() >= self.hold_until {
                if self.stream() {
                    keep_streaming = true;
                    if self.last_failure != 0 {
                        let since = seconds(self.now() - self.last_failure);
                        self.log(format_args!(
                            "USB streaming started, {since:.2} s after the last failure"
                        ));
                    } else {
                        self.log(format_args!("USB streaming started"));
                    }
                } else {
                    broken = true;
                }
            } else {
                // Idle: no transfers, but hotplug events still arrive here
                let mut done = Vec::new();
                let _ = self
                    .bus
                    .usb
                    .handle_events(Duration::from_millis(10), &mut done);
                self.bus
                    .deliver(&mut self.engine, done, Some(&mut self.audio));
                if self.bus.usb.gone() {
                    broken = true;
                }
            }
            if broken {
                let now = self.now();
                if failures.len() == RECOVERY_LIMIT {
                    if now - failures[0] < RECOVERY_WINDOW_NS {
                        self.log(format_args!(
                            "A16 failed {} times within {} s; giving up",
                            RECOVERY_LIMIT + 1,
                            RECOVERY_WINDOW_NS / 1_000_000_000
                        ));
                        result = 1;
                        break;
                    }
                    failures.remove(0);
                }
                failures.push(now);
                let boot_drop = wanted && !self.audio.booted && now < self.boot_until;
                let what = if boot_drop {
                    "A16 USB failure during its boot; resetting and reopening it"
                } else if self.bus.usb.gone() {
                    "A16 left the bus"
                } else {
                    "A16 USB failure; resetting and reopening it"
                };
                self.log(format_args!("{what}"));
                match self.recover(boot_drop, stopping) {
                    Recovery::Stopping => break,
                    Recovery::Lost => {
                        let present = self.bus.usb.find_a16();
                        self.log(format_args!(
                            "{}",
                            if present {
                                "A16 did not recover"
                            } else {
                                "A16 disconnected"
                            }
                        ));
                        result = i32::from(present);
                        break;
                    }
                    Recovery::Open => {}
                }
                self.last_failure = now;
                let reopened = self.now();
                self.log(format_args!(
                    "A16 reopened {:.2} s after the failure",
                    seconds(reopened - now)
                ));
                self.hold_until = reopened.max(now + RECOVERY_QUIET_NS);
                if boot_drop {
                    self.boot_until = self.hold_until + BOOT_EXTEND_NS;
                }
                self.log(format_args!(
                    "No USB requests until {:.2} s after the failure",
                    seconds(self.hold_until - now)
                ));
                continue;
            }
            if self.streaming && self.now() - last >= STATS_NS {
                let s = &self.shared;
                let line = format!(
                    "stats usb={}/{} pw={}/{} queues={}/{} xruns={},{}/{},{} ratios={:.7}/{:.7} graph={} rate_diff={:.7}",
                    self.bus.progress[0],
                    self.bus.progress[1],
                    s.sink.frames.load(Ordering::SeqCst),
                    s.source.frames.load(Ordering::SeqCst),
                    s.sink.fifo.available(),
                    s.source.fifo.available(),
                    s.sink.underruns.load(Ordering::SeqCst),
                    s.sink.overruns.load(Ordering::SeqCst),
                    s.source.underruns.load(Ordering::SeqCst),
                    s.source.overruns.load(Ordering::SeqCst),
                    f64::from_bits(s.output_ratio.load(Ordering::SeqCst)),
                    f64::from_bits(s.capture_ratio.load(Ordering::SeqCst)),
                    if s.graph_locked.load(Ordering::Relaxed) {
                        "a16"
                    } else {
                        "timer"
                    },
                    f64::from_bits(s.graph_rate_diff.load(Ordering::Relaxed)),
                );
                self.log(format_args!("{line}"));
                last = self.now();
            }
        }
        if self.shared.failed.load(Ordering::SeqCst) {
            result = 1;
        }
        result
    }

    /// The driver's USB lifecycle: open the A16, [`Bridge::run`], call `before_close`, and
    /// close the A16. The exit status is 0 also when no A16 is on the bus, so a restart
    /// policy does not retry until one appears.
    pub fn drive(
        &mut self,
        stopping: &AtomicBool,
        seconds_bound: u64,
        before_close: impl FnOnce(),
    ) -> i32 {
        let mut result = if self.open(false) {
            self.run(stopping, seconds_bound)
        } else {
            let present = self.bus.usb.find_a16();
            self.log(format_args!(
                "{}",
                if present {
                    "Cannot open the A16"
                } else {
                    "No A16 on the bus; exactly one high-speed A16 must be connected"
                }
            ));
            i32::from(present)
        };
        before_close();
        let was_configured = self.configured;
        self.close();
        if was_configured && self.bus.failed {
            result = 1;
        }
        result
    }
}

/// How a recovery ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Recovery {
    /// The A16 is open again.
    Open,
    /// The A16 is gone, or would not reopen.
    Lost,
    /// SIGINT or SIGTERM.
    Stopping,
}
