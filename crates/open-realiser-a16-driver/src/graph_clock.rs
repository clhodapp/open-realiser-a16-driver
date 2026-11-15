// SPDX-License-Identifier: MIT
//! The A16's clock driving the PipeWire graph: the frames the A16 has consumed and when,
//! published by the USB thread, and the cycle timer of the `a16.playback` driver, whose
//! wakeups and `rate_diff` a delay-locked loop corrects from those frames. The design is
//! that of PipeWire's ALSA sink, with the A16's consumed frames in place of the hardware
//! pointer: each cycle, the error is how far the graph's position has moved from the A16
//! position since the loop locked.
//!
//! Without an A16 clock (before USB streams, while the bridge recovers from a failure, or
//! when the USB thread stops publishing) the timer runs uncorrected at the graph's rate.

use std::f64::consts::PI;
use std::sync::atomic::{AtomicU64, Ordering, fence};

use crate::bridge::RATE;

/// An A16 sample older than this, at a cycle's start, means there is no A16 clock.
pub const STALE_NS: u64 = 100_000_000;
/// How long the A16 clock must stay fresh before the timer locks to it: the first
/// transfers of a stream complete in a burst while the A16 fills its buffer.
pub const SETTLE_NS: u64 = 200_000_000;
/// A wakeup further than this from the cycle's scheduled start restarts the timer from
/// the wakeup, as ALSA's driver does after an impossible timeout.
pub const LATE_NS: u64 = 1_000_000_000;
/// The delay-locked loop's widest bandwidth, from which it starts, and its narrowest:
/// `SPA_DLL_BW_MAX` and the ALSA driver's minimum.
pub const BW_MAX: f64 = 0.128;
pub const BW_MIN: f64 = 0.001;
/// How often the bandwidth adapts to the error's spread.
pub const BW_PERIOD_NS: u64 = 3_000_000_000;
/// An error larger than this many frames is a jump, not drift: the loop absorbs it into
/// its offset instead of steering toward it.
pub const MAX_RESYNC: f64 = 4096.0;
/// The largest correction of the cycle rate.
pub const MAX_CORRECTION: f64 = 0.005;

/// What the A16 had consumed by a moment: `frames` 48 kHz frames by `nsec`, on
/// `CLOCK_MONOTONIC`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    pub frames: u64,
    pub nsec: u64,
}

/// The latest [`Sample`], with one writer (the USB thread) and readers that never block it:
/// a sequence lock over atomics.
#[derive(Debug, Default)]
pub struct A16Clock {
    seq: AtomicU64,
    frames: AtomicU64,
    nsec: AtomicU64,
}

impl A16Clock {
    /// Writer: publish `sample`.
    pub fn publish(&self, sample: Sample) {
        let seq = self.seq.load(Ordering::Relaxed);
        self.seq.store(seq.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        self.frames.store(sample.frames, Ordering::Relaxed);
        self.nsec.store(sample.nsec, Ordering::Relaxed);
        self.seq.store(seq.wrapping_add(2), Ordering::Release);
    }

    /// Reader: the latest sample, or None before the first or while a write is in
    /// progress.
    pub fn read(&self) -> Option<Sample> {
        let before = self.seq.load(Ordering::Acquire);
        if before == 0 || before & 1 != 0 {
            return None;
        }
        let frames = self.frames.load(Ordering::Relaxed);
        let nsec = self.nsec.load(Ordering::Relaxed);
        fence(Ordering::Acquire);
        (self.seq.load(Ordering::Relaxed) == before).then_some(Sample { frames, nsec })
    }
}

/// PipeWire's `spa_dll`: a second-order delay-locked loop from a position error, in frames,
/// to a rate correction.
#[derive(Clone, Copy, Debug, Default)]
pub struct Dll {
    bw: f64,
    z1: f64,
    z2: f64,
    z3: f64,
    w0: f64,
    w1: f64,
    w2: f64,
}

impl Dll {
    /// Set the bandwidth for updates every `period` frames at `rate`, keeping the state.
    pub fn set_bw(&mut self, bw: f64, period: u64, rate: u32) {
        let w = 2.0 * PI * bw * period as f64 / f64::from(rate);
        self.w0 = 1.0 - (-20.0 * w).exp();
        self.w1 = w * 1.5 / period as f64;
        self.w2 = w / 1.5;
        self.bw = bw;
    }

    /// The correction for `err` frames ahead of the target.
    pub fn update(&mut self, err: f64) -> f64 {
        self.z1 += self.w0 * (self.w1 * err - self.z1);
        self.z2 += self.w0 * (self.z1 - self.z2);
        self.z3 += self.w2 * self.z2;
        1.0 - (self.z2 + self.z3)
    }
}

/// One cycle as the driver reports it in `SPA_IO_Clock`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cycle {
    /// The cycle's logical start.
    pub nsec: u64,
    /// The next cycle's logical start, when the timer fires next.
    pub next_nsec: u64,
    /// The A16's rate over the monotonic clock rate; 1.0 without an A16 clock.
    pub rate_diff: f64,
    /// The cycle follows the A16 clock.
    pub locked: bool,
}

/// The loop's reference since it locked.
#[derive(Clone, Copy, Debug)]
struct Lock {
    rate: u32,
    /// The A16's frames at the lock.
    frames: u64,
    /// The graph's frames since the lock, at the graph's rate.
    graph: f64,
    /// The graph's lead over the A16 at the lock, in graph frames.
    offset: f64,
}

/// The driver's cycle timer, on the data thread: when each cycle starts and its
/// `rate_diff`.
#[derive(Clone, Debug, Default)]
pub struct Cycles {
    dll: Dll,
    next_time: u64,
    base_time: u64,
    lock: Option<Lock>,
    last: Option<Sample>,
    fresh_since: Option<u64>,
    err_avg: f64,
    err_var: f64,
    /// The last error the loop steered by, in graph frames.
    pub err: f64,
    /// Jumps absorbed since the timer started.
    pub resyncs: u64,
    /// Locks to the A16 clock since the timer started.
    pub locks: u64,
}

impl Cycles {
    /// Start the timer afresh: the first cycle starts at `now`.
    pub fn start(&mut self, now: u64) {
        *self = Cycles {
            next_time: now,
            ..Cycles::default()
        };
    }

    /// The timer fired at `now` for a cycle of `duration` frames at `rate` Hz, with `sample`
    /// the latest A16 sample if one was read: the cycle that starts.
    pub fn wake(&mut self, now: u64, duration: u64, rate: u32, sample: Option<Sample>) -> Cycle {
        let duration = duration.max(1);
        let rate = if rate == 0 { RATE } else { rate };
        if self.next_time == 0 || now.abs_diff(self.next_time) > LATE_NS {
            self.next_time = now;
            self.lock = None;
            self.fresh_since = None;
        }
        let current = self.next_time;
        if let Some(sample) = sample {
            self.last = Some(sample);
        }
        let fresh = self
            .last
            .filter(|s| s.nsec.saturating_add(STALE_NS) > current);
        let mut corr = 1.0;
        match fresh {
            None => {
                self.lock = None;
                self.fresh_since = None;
            }
            Some(s) => {
                let since = *self.fresh_since.get_or_insert(current);
                if self
                    .lock
                    .is_some_and(|lock| lock.rate != rate || s.frames < lock.frames)
                {
                    self.lock = None;
                }
                if self.lock.is_none() && current.saturating_sub(since) >= SETTLE_NS {
                    self.lock_to(s, current, duration, rate);
                }
                if self.lock.is_some() {
                    corr = self.steer(s, current, duration, rate);
                }
            }
        }
        let next = current + (duration as f64 / corr * 1e9 / f64::from(rate)) as u64;
        self.next_time = next;
        if let Some(lock) = &mut self.lock {
            lock.graph += duration as f64;
        }
        Cycle {
            nsec: current,
            next_nsec: next,
            rate_diff: corr,
            locked: self.lock.is_some(),
        }
    }

    /// The A16's frames since the lock at `current`, at the graph's rate: `s` extrapolated
    /// at the nominal rate.
    fn a16_at(lock: &Lock, s: Sample, current: u64) -> f64 {
        let since = (s.frames - lock.frames) as f64;
        let ahead = (current as i64).wrapping_sub(s.nsec as i64) as f64 * f64::from(RATE) / 1e9;
        (since + ahead) * f64::from(lock.rate) / f64::from(RATE)
    }

    fn lock_to(&mut self, s: Sample, current: u64, duration: u64, rate: u32) {
        let mut lock = Lock {
            rate,
            frames: s.frames,
            graph: 0.0,
            offset: 0.0,
        };
        lock.offset = -Cycles::a16_at(&lock, s, current);
        self.lock = Some(lock);
        self.dll = Dll::default();
        self.dll.set_bw(BW_MAX, duration, rate);
        self.base_time = current;
        self.err_avg = 0.0;
        self.err_var = 0.0;
        self.locks += 1;
    }

    /// The correction for the cycle at `current`, as ALSA's `update_time` makes it.
    fn steer(&mut self, s: Sample, current: u64, duration: u64, rate: u32) -> f64 {
        let Some(lock) = &mut self.lock else {
            return 1.0;
        };
        let mut err = lock.graph - Cycles::a16_at(lock, s, current) - lock.offset;
        if err.abs() > MAX_RESYNC {
            lock.offset += err;
            err = 0.0;
            self.resyncs += 1;
        }
        let max_error = (duration as f64 / 2.0).max(256.0);
        err = err.clamp(-max_error, max_error);
        self.err = err;
        let corr = self.dll.update(err);
        let window = f64::from(rate) / duration as f64;
        let avg = (self.err_avg * window + (err - self.err_avg)) / (window + 1.0);
        self.err_var =
            (self.err_var * window + (err - self.err_avg) * (err - avg)) / (window + 1.0);
        self.err_avg = avg;
        if current - self.base_time > BW_PERIOD_NS {
            self.base_time = current;
            let bw = (self.err_avg.abs() + self.err_var.abs().sqrt()) / 1000.0;
            self.dll.set_bw(bw.clamp(BW_MIN, BW_MAX), duration, rate);
        }
        corr.clamp(1.0 - MAX_CORRECTION, 1.0 + MAX_CORRECTION)
    }
}

/// Completions an [`Earliest`] filter spans: about 107 ms of 80-frame transfers.
pub const EARLIEST_SPAN: usize = 64;

/// The USB thread's filter of completion times. A completion is reported late, never
/// early, so of the recent completions the one that came back least late dates the A16
/// frames best. A completion's time less its frames at the nominal rate is the A16 stream's
/// start plus that completion's lateness. The filter keeps the smallest of these over the
/// last [`EARLIEST_SPAN`] completions and dates the newest frames from it. Over that span
/// the A16's drift moves the dates by a few microseconds.
#[derive(Clone, Debug)]
pub struct Earliest {
    starts: [i64; EARLIEST_SPAN],
    count: usize,
    next: usize,
}

impl Default for Earliest {
    fn default() -> Earliest {
        Earliest {
            starts: [0; EARLIEST_SPAN],
            count: 0,
            next: 0,
        }
    }
}

impl Earliest {
    /// The sample for the completion at `nsec` that brought the A16's count to `frames`.
    pub fn date(&mut self, frames: u64, nsec: u64) -> Sample {
        let span = (frames as f64 * 1e9 / f64::from(RATE)) as i64;
        self.starts[self.next] = nsec as i64 - span;
        self.next = (self.next + 1) % EARLIEST_SPAN;
        self.count = (self.count + 1).min(EARLIEST_SPAN);
        let start = self.starts[..self.count]
            .iter()
            .copied()
            .min()
            .unwrap_or(nsec as i64 - span);
        Sample {
            frames,
            nsec: (start + span).max(0) as u64,
        }
    }

    /// Forget the completions so far, for a stream that starts again.
    pub fn reset(&mut self) {
        self.count = 0;
        self.next = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: u64 = 1_000_000_000;

    /// An A16 whose out transfers of 80 frames complete at a rate `ppm` off 48 kHz, each
    /// reported up to `jitter_ns` late, from `start` on.
    struct FakeA16 {
        start: u64,
        ppm: f64,
        jitter_ns: u64,
        /// Frames counted before `start`.
        frames0: u64,
        n: u64,
        at: u64,
        earliest: Earliest,
        published: Option<Sample>,
    }

    fn fake(start: u64, ppm: f64, jitter_ns: u64, frames0: u64) -> FakeA16 {
        FakeA16 {
            start,
            ppm,
            jitter_ns,
            frames0,
            n: 0,
            at: 0,
            earliest: Earliest::default(),
            published: None,
        }
    }

    impl FakeA16 {
        fn ideal(&self, n: u64) -> u64 {
            self.start
                + (n as f64 * 80.0 * 1e9 / (f64::from(RATE) * (1.0 + self.ppm * 1e-6))) as u64
        }

        /// A deterministic lateness for completion `n`.
        fn late(&self, n: u64) -> u64 {
            if self.jitter_ns == 0 {
                return 0;
            }
            let mut x = n
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            x ^= x >> 33;
            x % self.jitter_ns
        }

        fn reported(&self, n: u64) -> u64 {
            self.ideal(n) + self.late(n)
        }

        /// The newest sample the USB thread has published by `now`, through its filter.
        fn sample(&mut self, now: u64) -> Option<Sample> {
            while self.reported(self.n).max(self.at) <= now {
                self.at = self.reported(self.n).max(self.at);
                self.n += 1;
                self.published = Some(self.earliest.date(self.frames0 + self.n * 80, self.at));
            }
            self.published
        }
    }

    /// Run the timer from `from` to `until`, waking up to 200 µs after each scheduled
    /// start: every cycle.
    fn run(
        cycles: &mut Cycles,
        from: u64,
        until: u64,
        duration: u64,
        a16: &mut dyn FnMut(u64) -> Option<Sample>,
    ) -> Vec<Cycle> {
        let mut out = Vec::new();
        let mut wake = from;
        let mut k = 0u64;
        while wake < until {
            let c = cycles.wake(wake, duration, RATE, a16(wake));
            out.push(c);
            k += 1;
            wake = c.next_nsec + (k * 7919 % 200) * 1000;
        }
        out
    }

    /// The A16's rate over the monotonic clock.
    fn ratio(ppm: f64) -> f64 {
        1.0 + ppm * 1e-6
    }

    #[test]
    fn converges_to_the_a16_rate_through_jitter() {
        for ppm in [-300.0, -40.0, 0.0, 150.0, 300.0] {
            let mut a16 = fake(S, ppm, 1_500_000, 0);
            let mut cycles = Cycles::default();
            cycles.start(S);
            let all = run(&mut cycles, S, 181 * S, 1024, &mut |t| a16.sample(t));
            assert_eq!(cycles.locks, 1);
            assert_eq!(cycles.resyncs, 0);
            let settled: Vec<&Cycle> = all.iter().filter(|c| c.nsec > 120 * S).collect();
            assert!(settled.iter().all(|c| c.locked));
            // Each cycle's rate wanders with the completions' jitter; its mean is the A16's
            for c in &settled {
                assert!(
                    (c.rate_diff - ratio(ppm)).abs() < 5e-6,
                    "ppm {ppm}: rate_diff {}",
                    c.rate_diff
                );
            }
            let mean = settled.iter().map(|c| c.rate_diff).sum::<f64>() / settled.len() as f64;
            assert!((mean - ratio(ppm)).abs() < 1e-6, "ppm {ppm}: mean {mean}");
            // The graph's cycles cover the A16's frames: over the last minute the graph
            // moved within a few frames of the A16
            let (first, last) = (settled[0], settled[settled.len() - 1]);
            let graph = (settled.len() - 1) as f64 * 1024.0;
            let a16_frames = (last.nsec - first.nsec) as f64 * 48e-6 * ratio(ppm);
            assert!(
                (graph - a16_frames).abs() < 64.0,
                "ppm {ppm}: graph {graph} a16 {a16_frames}"
            );
            assert!(cycles.err.abs() < 128.0, "ppm {ppm}: err {}", cycles.err);
        }
    }

    #[test]
    fn holds_the_nominal_rate_without_a16_clock() {
        let mut cycles = Cycles::default();
        cycles.start(5 * S);
        let all = run(&mut cycles, 5 * S, 15 * S, 512, &mut |_| None);
        assert!(all.len() > 900);
        for c in &all {
            assert!(!c.locked);
            assert_eq!(c.rate_diff, 1.0);
            assert_eq!(c.next_nsec - c.nsec, 512 * S / u64::from(RATE));
        }
        // Each cycle starts when the one before scheduled it, however late the wakeup
        for w in all.windows(2) {
            assert_eq!(w[1].nsec, w[0].next_nsec);
        }
    }

    #[test]
    fn falls_back_during_a_gap_and_locks_again() {
        let mut before = fake(S, 250.0, 1_000_000, 0);
        // The A16 stops for 11 s, as during a recovery, then streams again with the counter
        // carrying on
        let mut after = fake(72 * S, 250.0, 1_000_000, 10_000_000);
        let mut a16 = |t: u64| {
            if t < 61 * S {
                before.sample(t)
            } else {
                after.sample(t)
            }
        };
        let mut cycles = Cycles::default();
        cycles.start(S);
        let all = run(&mut cycles, S, 200 * S, 1024, &mut a16);
        let stale_until = 61 * S + STALE_NS;
        for c in &all {
            if (stale_until..72 * S).contains(&c.nsec) {
                assert!(!c.locked);
                assert_eq!(c.rate_diff, 1.0);
            }
            if c.nsec > 72 * S + SETTLE_NS + 50_000_000 {
                assert!(c.locked, "at {}", c.nsec);
            }
            if c.nsec > 140 * S {
                assert!((c.rate_diff - ratio(250.0)).abs() < 5e-6, "{}", c.rate_diff);
            }
        }
        // Before the gap it had converged too
        let before_gap = all.iter().rfind(|c| c.nsec < 60 * S).expect("cycles");
        assert!((before_gap.rate_diff - ratio(250.0)).abs() < 5e-6);
        assert_eq!(cycles.locks, 2);
    }

    #[test]
    fn a_quantum_change_is_no_jump() {
        let mut a16 = fake(S, -120.0, 500_000, 0);
        let mut cycles = Cycles::default();
        cycles.start(S);
        let mut all = run(&mut cycles, S, 60 * S, 1024, &mut |t| a16.sample(t));
        let resume = all[all.len() - 1].next_nsec;
        all.extend(run(&mut cycles, resume, 120 * S, 256, &mut |t| {
            a16.sample(t)
        }));
        assert_eq!(cycles.locks, 1);
        assert_eq!(cycles.resyncs, 0);
        assert!(cycles.err.abs() < 128.0);
        let last = all[all.len() - 1];
        assert!((last.rate_diff - ratio(-120.0)).abs() < 5e-6);
    }

    #[test]
    fn a_late_wakeup_restarts_the_timer() {
        let mut cycles = Cycles::default();
        cycles.start(S);
        let c = cycles.wake(S, 1024, RATE, None);
        let late = c.next_nsec + 3 * S;
        let d = cycles.wake(late, 1024, RATE, None);
        assert_eq!(d.nsec, late);
    }

    #[test]
    fn the_clock_reads_what_was_published() {
        let clock = A16Clock::default();
        assert_eq!(clock.read(), None);
        let s = Sample {
            frames: 80,
            nsec: 12345,
        };
        clock.publish(s);
        assert_eq!(clock.read(), Some(s));
    }

    #[test]
    fn concurrent_reads_see_whole_samples() {
        use std::sync::Arc;
        let clock = Arc::new(A16Clock::default());
        let writer = {
            let clock = Arc::clone(&clock);
            std::thread::spawn(move || {
                for i in 1..=200_000u64 {
                    clock.publish(Sample {
                        frames: i * 80,
                        nsec: i * 1_666_667,
                    });
                }
            })
        };
        let mut seen = 0;
        while seen < 200_000 * 80 {
            if let Some(s) = clock.read() {
                assert_eq!(s.nsec, s.frames / 80 * 1_666_667);
                assert!(s.frames >= seen);
                seen = s.frames;
            }
        }
        writer.join().expect("writer");
    }
}
