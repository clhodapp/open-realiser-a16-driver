// SPDX-License-Identifier: MIT
//! The equivalence check against the C engine this crate ports: the engine on a fake bus
//! through five scenarios, every control request (with its data), clear-halt, sleep,
//! submission (with the bytes of out transfers), completion and progress report, a hash of
//! the in ring after each in completion, and the stream's state after each start. The text
//! is byte for byte what the C harness printed for the C engine at the commit the port
//! started from; its SHA-256 is recorded here.
//!
//! Each scenario: start, 300 transfers, suspend and start, rate restarts to 44.1 and 96 kHz,
//! new transfer sizes and queue and a restart at 48 kHz, the rings detached, stop. The
//! scenarios: the A16's interrupt endpoints at 48 kHz, at no rate requested, its bulk layout,
//! and two nonzero 'I' register answers.
//!
//! Set `A16_TRACE_OUT` to a path to write the trace there.

#[path = "support/sha256.rs"]
mod sha256;

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::time::Duration;

use a16_engine::{
    Bus, BusError, ControlSetup, Direction, Engine, EngineState, INTERFACES, PRODUCT_NAME, PcmRing,
    SampleRate, Setting, SettingChange, SettingError, Status, Transfer, UsbEndpoint,
};

/// The digest of the C harness's output for the C engine (18,220,690 bytes).
const C_TRACE_SHA256: &str = "c90a0014418236dfcf6f731869b42ac760993ece87b1a2f98217929166dbdec5";
const C_TRACE_BYTES: usize = 18_220_690;

/// What the C harness takes as the shared memory's ring: 1 MiB of transfers end to end.
const C_RING_BYTES: usize = 1024 * 1024;

fn hex(out: &mut String, bytes: &[u8]) {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    out.reserve(bytes.len() * 2);
    for &b in bytes {
        out.push(DIGITS[usize::from(b >> 4)] as char);
        out.push(DIGITS[usize::from(b & 15)] as char);
    }
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(1469598103934665603u64, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(1099511628211)
    })
}

struct TraceBus {
    out: String,
    queue: VecDeque<Transfer>,
    rate: [u8; 3],
    now: Duration,
    /// What an 'I' read answers, plus its wIndex.
    i_val: u8,
}

impl Bus for TraceBus {
    fn submit(&mut self, transfer: Transfer) -> Result<(), Transfer> {
        let _ = write!(
            self.out,
            "submit {:02x} size={} tok={}",
            transfer.endpoint(),
            transfer.len(),
            transfer.id()
        );
        if transfer.direction() == Direction::Out {
            self.out.push_str(" data=");
            hex(&mut self.out, transfer.buffer());
        }
        self.out.push('\n');
        self.queue.push_back(transfer);
        Ok(())
    }

    fn control_in(&mut self, setup: ControlSetup, data: &mut [u8]) -> Result<(), BusError> {
        data.fill(0);
        match (setup.request_type, setup.request) {
            (0xA2, 0x81) if data.len() >= 3 => data[..3].copy_from_slice(&self.rate),
            (0xC0, 0x56) if data.len() >= 3 => data[..3].copy_from_slice(&[0x02, 0x01, 0x04]),
            (0xC0, 0x49) if !data.is_empty() => {
                data[0] = self.i_val.wrapping_add(setup.index as u8);
            }
            _ => {}
        }
        let _ = write!(
            self.out,
            "control {:02x} {:02x} {:04x} {:04x} {} in=",
            setup.request_type,
            setup.request,
            setup.value,
            setup.index,
            data.len()
        );
        hex(&mut self.out, data);
        self.out.push('\n');
        Ok(())
    }

    fn control_out(&mut self, setup: ControlSetup, data: &[u8]) -> Result<(), BusError> {
        if setup.request_type == 0x22 && setup.request == 0x01 && data.len() >= 3 {
            self.rate.copy_from_slice(&data[..3]);
        }
        let _ = write!(
            self.out,
            "control {:02x} {:02x} {:04x} {:04x} {} out=",
            setup.request_type,
            setup.request,
            setup.value,
            setup.index,
            data.len()
        );
        hex(&mut self.out, data);
        self.out.push('\n');
        Ok(())
    }

    fn clear_halt(&mut self, endpoint: u8) -> Result<(), BusError> {
        let _ = writeln!(self.out, "clear_halt {endpoint:02x}");
        Ok(())
    }

    fn sleep(&mut self, duration: Duration) {
        let _ = writeln!(self.out, "sleep {}", duration.as_millis());
        self.now += duration;
    }

    fn now(&mut self) -> Duration {
        self.now += Duration::from_micros(125);
        self.now
    }

    fn abort(&mut self) -> Vec<Transfer> {
        let aborted: Vec<Transfer> = self.queue.drain(..).collect();
        let _ = writeln!(self.out, "abort {}", aborted.len());
        aborted
    }

    fn pcm_progress(&mut self, direction: Direction, position: u64, _ring: &mut PcmRing) {
        let dir = match direction {
            Direction::Out => 0,
            Direction::In => 1,
        };
        let _ = writeln!(self.out, "progress {dir} {position}");
    }
}

struct Scenario {
    engine: Engine,
    bus: TraceBus,
    in_count: u32,
    /// The in ring once detached, which the hash keeps reading.
    detached_in: Option<PcmRing>,
}

impl Scenario {
    fn in_ring_hash(&self) -> u64 {
        let ring = self
            .engine
            .pcm_ring(Direction::In)
            .or(self.detached_in.as_ref());
        fnv(ring.expect("an in ring").as_bytes())
    }

    fn run_transfers(&mut self, n: usize) {
        for _ in 0..n {
            let Some(mut transfer) = self.bus.queue.pop_front() else {
                break;
            };
            let mut bytes = transfer.len();
            let is_in = transfer.direction() == Direction::In;
            if is_in {
                let c = self.in_count;
                self.in_count += 1;
                for (k, b) in transfer.buffer_mut().iter_mut().enumerate() {
                    *b = c
                        .wrapping_mul(31)
                        .wrapping_add(k as u32 * 7)
                        .wrapping_add(3) as u8;
                }
                if c % 17 == 9 {
                    bytes /= 2; // a short in transfer
                }
                if c % 41 == 20 {
                    bytes = 0;
                }
            }
            let _ = writeln!(
                self.bus.out,
                "complete {:02x} tok={} bytes={bytes}",
                transfer.endpoint(),
                transfer.id()
            );
            self.engine
                .complete(&mut self.bus, transfer, Status::Completed(bytes));
            if is_in {
                let hash = self.in_ring_hash();
                let _ = writeln!(self.bus.out, "in_ring {hash:016x}");
            }
        }
    }

    fn state(&mut self, what: &str, ok: bool) {
        let e = &self.engine;
        let ring = |d| C_RING_BYTES / e.transfer_bytes(d) * e.transfer_frames(d) as usize;
        let _ = writeln!(
            self.bus.out,
            "{what} {} rate={} out_frames={} in_frames={} obpf={} ibpf={} safety={}/{} ring={}/{} in={} out={}",
            u8::from(ok),
            e.sample_rate().map_or(0, SampleRate::hz),
            e.transfer_frames(Direction::Out),
            e.transfer_frames(Direction::In),
            Direction::Out.wire_frame_bytes(),
            Direction::In.wire_frame_bytes(),
            e.safety_offset(Direction::Out),
            e.safety_offset(Direction::In),
            ring(Direction::Out),
            ring(Direction::In),
            Direction::In.channels(),
            Direction::Out.channels(),
        );
    }

    fn restart(&mut self, what: &str) {
        let ok = self.engine.restart(&mut self.bus).is_ok();
        self.state(what, ok);
    }

    fn change_rate(&mut self, hz: u32) {
        let rate = SampleRate::from_hz(hz).expect("a rate the A16 lists");
        let code = change_code(Ok(self.engine.change_rate(rate)));
        let _ = writeln!(self.bus.out, "change_rate {code}");
    }

    fn change_setting(&mut self, label: &str, setting: Setting, value: u8) {
        let code = change_code(self.engine.change_setting(setting, value));
        let _ = writeln!(self.bus.out, "{label} {code}");
    }
}

/// A change's result as the C engine's enum ozzy_setting_change numbers it.
fn change_code(change: Result<SettingChange, SettingError>) -> i32 {
    match change {
        Err(_) => -1,
        Ok(SettingChange::Unchanged) => 0,
        Ok(SettingChange::NeedsRestart) => 2,
    }
}

fn scenario(out: &mut String, interrupt: bool, first_rate: u32, i_val: u8) {
    let _ = writeln!(
        out,
        "==== {PRODUCT_NAME} {} first_rate={first_rate}",
        if interrupt { "interrupt" } else { "bulk" }
    );
    let mut engine = Engine::new();
    engine.set_logger(Box::new(|_| {}));
    let _ = writeln!(
        out,
        "profile intf={} alt={}/{}",
        INTERFACES.len(),
        INTERFACES[0].alt_setting,
        INTERFACES[1].alt_setting
    );
    let attributes = if interrupt { 3 } else { 2 };
    let endpoints = [
        UsbEndpoint {
            interface: 0,
            address: 0x05,
            attributes,
            interval: 1,
            max_packet: if interrupt { 964 } else { 512 },
        },
        UsbEndpoint {
            interface: 0,
            address: 0x83,
            attributes,
            interval: 1,
            max_packet: 64,
        },
        UsbEndpoint {
            interface: 1,
            address: 0x86,
            attributes,
            interval: 1,
            max_packet: if interrupt { 1024 } else { 512 },
        },
    ];
    let bound = engine.bind_endpoints(&endpoints);
    for (role, direction) in Direction::ALL.into_iter().enumerate() {
        let address = bound[direction].map_or(0, |ep| ep.address);
        let _ = writeln!(out, "bound {role} {address:02x}");
    }
    engine.set_rate(SampleRate::from_hz(first_rate));
    let out_ring: Vec<u8> = (0..256 * 16 * 3usize)
        .map(|k| (k * 13 + 5 + (k >> 8)) as u8)
        .collect();
    engine.attach_pcm_ring(PcmRing::from_bytes(Direction::Out, out_ring), 0);
    engine.attach_pcm_ring(PcmRing::new(Direction::In, 256), 0);

    let mut s = Scenario {
        engine,
        bus: TraceBus {
            out: std::mem::take(out),
            queue: VecDeque::new(),
            rate: [0; 3],
            now: Duration::ZERO,
            i_val,
        },
        in_count: 0,
        detached_in: None,
    };
    let ok = s.engine.start(&mut s.bus).is_ok();
    s.state("start", ok);
    s.run_transfers(300);

    // The driver's pause and stream
    s.engine.suspend(&mut s.bus);
    let ok = s.engine.start(&mut s.bus).is_ok();
    s.state("resume", ok);
    s.run_transfers(120);

    // Rate restarts
    s.change_rate(44100);
    s.restart("restart44");
    s.run_transfers(200);
    s.change_rate(96000);
    s.restart("restart96");
    s.run_transfers(150);

    // Transfer sizes and queues
    s.change_setting("set out frames", Setting::PcmOutFrames, 5);
    s.change_setting("set in frames", Setting::PcmInFrames, 4);
    s.change_setting("set out queue", Setting::PcmOutQueue, 1);
    s.change_rate(48000);
    s.restart("restart48");
    s.run_transfers(150);

    // The rings detached: silence out
    s.engine.detach_pcm_ring(Direction::Out);
    s.detached_in = s.engine.detach_pcm_ring(Direction::In);
    s.bus.out.push_str("detached\n");
    s.run_transfers(60);
    s.engine.stop(&mut s.bus);
    let published = u8::from(s.engine.state() != EngineState::Stopped);
    let _ = writeln!(s.bus.out, "stopped hw={published} ready={published}");
    *out = s.bus.out;
}

#[test]
fn trace_matches_c_reference() {
    let mut out = String::with_capacity(C_TRACE_BYTES);
    scenario(&mut out, true, 48000, 0);
    scenario(&mut out, true, 0, 0);
    scenario(&mut out, false, 48000, 0);
    scenario(&mut out, true, 48000, 0x23);
    scenario(&mut out, true, 44100, 0xFF);

    if let Some(path) = std::env::var_os("A16_TRACE_OUT") {
        std::fs::write(&path, &out).expect("write the trace");
    }
    let digest = sha256::hex_digest(out.as_bytes());
    if digest != C_TRACE_SHA256 {
        let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("a16.trace");
        std::fs::write(&path, &out).expect("write the trace");
        panic!(
            "trace differs from the C reference trace: {} bytes, sha256 {digest}, written to {}",
            out.len(),
            path.display()
        );
    }
    assert_eq!(out.len(), C_TRACE_BYTES);
}
