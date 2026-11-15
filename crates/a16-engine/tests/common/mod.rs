// SPDX-License-Identifier: MIT
//! A fake USB bus for the engine tests: transfers queue up and complete when the test says
//! so, control requests answer like the A16 (the rate read returns the rate last set), and
//! everything the engine does can be traced, one line per call.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::time::Duration;

use a16_engine::{
    Bus, BusError, ControlSetup, Direction, Engine, PcmRing, PerDirection, Pipe, Status, Transfer,
    TransferType,
};

#[derive(Default)]
pub struct FakeBus {
    pub queue: VecDeque<Transfer>,
    /// The last UAC1 SET_CUR rate.
    pub rate: [u8; 3],
    /// What an 'I' read of wIndex 0-3 answers.
    pub i_reg: [u8; 4],
    /// The clock: it ticks a nanosecond per read unless `fixed_clock`, which leaves it to the
    /// test.
    pub now: Duration,
    pub fixed_clock: bool,
    pub stats_changed: u32,
    /// The last pcm_progress per direction.
    pub progress: PerDirection<u64>,
    /// One line per call, if tracing.
    pub trace: Option<String>,
}

impl FakeBus {
    pub fn new() -> FakeBus {
        FakeBus::default()
    }

    pub fn traced() -> FakeBus {
        FakeBus {
            trace: Some(String::new()),
            ..FakeBus::default()
        }
    }

    fn line(&mut self, args: std::fmt::Arguments<'_>) {
        if let Some(trace) = &mut self.trace {
            let _ = trace.write_fmt(args);
            trace.push('\n');
        }
    }

    pub fn pop(&mut self) -> Option<Transfer> {
        self.queue.pop_front()
    }

    /// Transfers queued.
    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// Complete `transfer` in full; an in transfer gets `fill` first.
    pub fn complete(
        &mut self,
        engine: &mut Engine,
        mut transfer: Transfer,
        fill: impl FnOnce(&mut Transfer),
    ) {
        if transfer.direction() == Direction::In {
            fill(&mut transfer);
        }
        let bytes = transfer.len();
        engine.complete(self, transfer, Status::Completed(bytes));
    }

    /// Pop and complete one transfer, as it is.
    pub fn complete_next(&mut self, engine: &mut Engine) -> bool {
        match self.pop() {
            Some(transfer) => {
                self.complete(engine, transfer, |_| {});
                true
            }
            None => false,
        }
    }
}

impl Bus for FakeBus {
    fn submit(&mut self, transfer: Transfer) -> Result<(), Transfer> {
        self.line(format_args!(
            "submit {:02x} size={} xfer={}",
            transfer.endpoint(),
            transfer.len(),
            transfer.id()
        ));
        self.queue.push_back(transfer);
        Ok(())
    }

    fn control_in(&mut self, setup: ControlSetup, data: &mut [u8]) -> Result<(), BusError> {
        self.line(format_args!(
            "control {:02x} {:02x} {:04x} {:04x} {}",
            setup.request_type,
            setup.request,
            setup.value,
            setup.index,
            data.len()
        ));
        data.fill(0);
        match (setup.request_type, setup.request) {
            (0xA2, 0x81) if data.len() >= 3 => data[..3].copy_from_slice(&self.rate),
            (0xC0, 0x49) if !data.is_empty() && usize::from(setup.index) < self.i_reg.len() => {
                data[0] = self.i_reg[usize::from(setup.index)];
            }
            _ => {}
        }
        Ok(())
    }

    fn control_out(&mut self, setup: ControlSetup, data: &[u8]) -> Result<(), BusError> {
        self.line(format_args!(
            "control {:02x} {:02x} {:04x} {:04x} {}",
            setup.request_type,
            setup.request,
            setup.value,
            setup.index,
            data.len()
        ));
        if setup.request_type == 0x22 && setup.request == 0x01 && data.len() >= 3 {
            self.rate.copy_from_slice(&data[..3]);
        }
        Ok(())
    }

    fn clear_halt(&mut self, endpoint: u8) -> Result<(), BusError> {
        self.line(format_args!("clear_halt {endpoint:02x}"));
        Ok(())
    }

    fn sleep(&mut self, _duration: Duration) {}

    fn now(&mut self) -> Duration {
        if !self.fixed_clock {
            self.now += Duration::from_nanos(1);
        }
        self.now
    }

    fn abort(&mut self) -> Vec<Transfer> {
        let aborted: Vec<Transfer> = self.queue.drain(..).collect();
        self.line(format_args!("abort {}", aborted.len()));
        aborted
    }

    fn pcm_progress(&mut self, direction: Direction, position: u64, _ring: &mut PcmRing) {
        let dir = match direction {
            Direction::Out => 0,
            Direction::In => 1,
        };
        self.line(format_args!("progress {dir} {position}"));
        self.progress[direction] = position;
    }

    fn stats_changed(&mut self) {
        self.stats_changed += 1;
    }
}

/// An engine that logs nothing.
pub fn quiet_engine() -> Engine {
    let mut engine = Engine::new();
    engine.set_logger(Box::new(|_| {}));
    engine
}

pub const fn pipe(transfer_type: TransferType, max_packet_size: u16, interval: u8) -> Pipe {
    Pipe {
        transfer_type,
        max_packet_size,
        interval,
    }
}

/// An engine on the A16's pipes: 964-byte interrupt out, 1024-byte interrupt in.
pub fn a16_engine() -> Engine {
    let mut engine = quiet_engine();
    engine.set_pipe(Direction::Out, pipe(TransferType::Interrupt, 964, 1));
    engine.set_pipe(Direction::In, pipe(TransferType::Interrupt, 1024, 1));
    engine
}

/// FNV-1a, as the C tests hash.
pub fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(1469598103934665603u64, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(1099511628211)
    })
}

/// The C tests' small deterministic generator.
pub fn fake_rand(state: &mut u32) -> u32 {
    *state = state.wrapping_mul(1664525).wrapping_add(1013904223);
    *state >> 8
}
