// SPDX-License-Identifier: MIT
//! What moves audio between PipeWire's threads and the USB thread: the single-producer,
//! single-consumer queues, the 24-bit sample conversion, the queue-level feedback that steers
//! the resamplers, and the level check on the A16's return.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU32, Ordering};

/// Frames a queue holds.
pub const CAPACITY: u32 = 16384;
/// The queue level each side aims for, and the level a queue fills to before it plays.
pub const TARGET: u32 = 2048;
/// The most channels a queue frame has.
pub const CHANNELS: usize = 16;

/// A queue of interleaved float frames with one producer and one consumer. Only the owner of
/// an index advances it: the producer the write index, the consumer the read index.
pub struct Fifo {
    read: AtomicU32,
    write: AtomicU32,
    channels: usize,
    data: Box<[UnsafeCell<f32>]>,
}

// The producer writes only frames the consumer has released, and the consumer reads only
// frames the producer has published, each ordered by the index it acquires.
unsafe impl Sync for Fifo {}

impl Fifo {
    /// An empty queue of `channels`-channel frames.
    pub fn new(channels: usize) -> Fifo {
        assert!((1..=CHANNELS).contains(&channels));
        Fifo {
            read: AtomicU32::new(0),
            write: AtomicU32::new(0),
            channels,
            data: (0..CAPACITY as usize * CHANNELS)
                .map(|_| UnsafeCell::new(0.0))
                .collect(),
        }
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Frames queued.
    pub fn available(&self) -> u32 {
        let r = self.read.load(Ordering::Acquire);
        let w = self.write.load(Ordering::Acquire);
        w.wrapping_sub(r)
    }

    fn slot(&self, frame: u32) -> *mut f32 {
        let at = (frame % CAPACITY) as usize * self.channels;
        self.data[at].get()
    }

    /// Producer: queue up to `src.len() / channels` frames, as many as fit; returns how many.
    pub fn push(&self, src: &[f32]) -> u32 {
        let n = (src.len() / self.channels) as u32;
        let w = self.write.load(Ordering::Relaxed);
        let r = self.read.load(Ordering::Acquire);
        let count = n.min(CAPACITY - w.wrapping_sub(r));
        for i in 0..count {
            let from = &src[i as usize * self.channels..(i as usize + 1) * self.channels];
            // SAFETY: the frame is outside what the consumer may read until the store below
            unsafe {
                std::ptr::copy_nonoverlapping(
                    from.as_ptr(),
                    self.slot(w.wrapping_add(i)),
                    self.channels,
                );
            }
        }
        self.write.store(w.wrapping_add(count), Ordering::Release);
        count
    }

    /// Consumer: copy up to `dst.len() / channels` queued frames, without consuming them.
    pub fn peek(&self, dst: &mut [f32]) -> u32 {
        let n = (dst.len() / self.channels) as u32;
        let r = self.read.load(Ordering::Relaxed);
        let w = self.write.load(Ordering::Acquire);
        let count = n.min(w.wrapping_sub(r));
        for i in 0..count {
            let to = &mut dst[i as usize * self.channels..(i as usize + 1) * self.channels];
            // SAFETY: the producer published the frame and leaves it until the consumer
            // releases it
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.slot(r.wrapping_add(i)),
                    to.as_mut_ptr(),
                    self.channels,
                );
            }
        }
        count
    }

    /// Consumer: release `n` frames.
    pub fn consume(&self, n: u32) {
        let r = self.read.load(Ordering::Relaxed);
        self.read.store(r.wrapping_add(n), Ordering::Release);
    }

    /// Consumer: release every frame queued.
    pub fn discard(&self) {
        let w = self.write.load(Ordering::Acquire);
        self.read.store(w, Ordering::Release);
    }

    /// Both indices at `index`, for a test of the counters' wrap.
    #[doc(hidden)]
    pub fn set_indices(&self, index: u32) {
        self.read.store(index, Ordering::SeqCst);
        self.write.store(index, Ordering::SeqCst);
    }
}

/// An S24_3LE sample as a float in [-1, 1).
pub fn unpack(p: &[u8]) -> f32 {
    let u = u32::from(p[0]) | u32::from(p[1]) << 8 | u32::from(p[2]) << 16;
    let v = if u & 0x80_0000 != 0 {
        u | 0xff00_0000
    } else {
        u
    } as i32;
    v as f32 / 8_388_608.0
}

/// The largest float below 1.
const MAX_SAMPLE: f32 = 1.0 - f32::EPSILON;

/// A float as an S24_3LE sample: clipped to the 24-bit range, nonfinite values as silence.
pub fn pack(p: &mut [u8], v: f32) {
    let v = if v.is_finite() { v } else { 0.0 }.clamp(-1.0, MAX_SAMPLE);
    let s = (v * 8_388_608.0).round_ties_even() as i32;
    let u = s as u32;
    p[0] = u as u8;
    p[1] = (u >> 8) as u8;
    p[2] = (u >> 16) as u8;
}

/// The feedback from a queue's level to its resampler's ratio.
#[derive(Clone, Copy, Debug, Default)]
pub struct Clock {
    filtered: f64,
    integral: f64,
    initialized: bool,
}

impl Clock {
    /// The resampling ratio for a queue at `level` frames, `seconds` after the last call:
    /// smoothed, with a bounded integral, within 0.5% of 1.
    pub fn ratio(&mut self, level: u32, seconds: f64) -> f64 {
        let target = f64::from(TARGET);
        if !self.initialized {
            self.filtered = target;
            self.initialized = true;
        }
        let alpha = (seconds * 4.0).min(1.0);
        self.filtered += alpha * (f64::from(level) - self.filtered);
        let error = self.filtered - target;
        self.integral = (self.integral + error * seconds * 0.000_000_01).clamp(-0.002, 0.002);
        let correction = (error * 0.000_001 + self.integral).clamp(-0.005, 0.005);
        1.0 - correction
    }
}

/// Frames in one window of the level check: 100 ms at 48 kHz.
pub const LEVEL_FRAMES: u32 = 4800;
/// Consecutive windows that must reach the level.
pub const LEVEL_WINDOWS: u32 = 3;
/// The RMS level, in dBFS, that a window must reach.
pub const LEVEL_DBFS: f64 = -55.0;

/// Sustained level detection on the A16's return: reached once [`LEVEL_WINDOWS`] consecutive
/// windows of [`LEVEL_FRAMES`] frames each have an RMS of at least [`LEVEL_DBFS`]. A silent
/// return carries sparse single-sample spikes, which stay well below that in any window.
#[derive(Clone, Copy, Debug, Default)]
pub struct Level {
    sum: f64,
    frames: u32,
    loud: u32,
    reached: bool,
}

impl Level {
    /// Feed interleaved frames of `channels` float samples; true once the level is reached.
    pub fn feed(&mut self, samples: &[f32], channels: usize) -> bool {
        let threshold = 10f64.powf(LEVEL_DBFS / 20.0);
        for frame in samples.chunks_exact(channels) {
            if self.reached {
                break;
            }
            for &s in frame {
                self.sum += f64::from(s) * f64::from(s);
            }
            self.frames += 1;
            if self.frames < LEVEL_FRAMES {
                continue;
            }
            let rms = (self.sum / (f64::from(self.frames) * channels as f64)).sqrt();
            self.loud = if rms >= threshold { self.loud + 1 } else { 0 };
            self.reached = self.loud >= LEVEL_WINDOWS;
            self.sum = 0.0;
            self.frames = 0;
        }
        self.reached
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn concurrent_order_across_the_counter_wrap() {
        let fifo = Arc::new(Fifo::new(2));
        fifo.set_indices(u32::MAX - 10000);
        let producer = {
            let fifo = Arc::clone(&fifo);
            std::thread::spawn(move || {
                for i in 0..200_000u32 {
                    let frame = [i as f32, -(i as f32)];
                    while fifo.push(&frame) == 0 {}
                }
            })
        };
        for i in 0..200_000u32 {
            let mut frame = [0.0; 2];
            while fifo.peek(&mut frame) == 0 {}
            assert_eq!(frame, [i as f32, -(i as f32)]);
            fifo.consume(1);
        }
        producer.join().expect("producer");
        assert_eq!(fifo.available(), 0);
        let block = vec![0.0; CAPACITY as usize * 2];
        assert_eq!(fifo.push(&block), CAPACITY);
        assert_eq!(fifo.push(&[0.0, 0.0]), 0);
        fifo.discard();
        assert_eq!(fifo.available(), 0);
    }

    #[test]
    fn pcm_clipping() {
        for v in [
            -1.0,
            0.0,
            0.5,
            MAX_SAMPLE,
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            2.0,
            -2.0,
        ] {
            let mut bytes = [0; 3];
            pack(&mut bytes, v);
            let back = unpack(&bytes);
            assert!(back.is_finite() && (-1.0..1.0).contains(&back));
            if v.is_finite() && v.abs() <= 1.0 {
                assert!((back - v).abs() < 0.000_000_13);
            }
        }
    }

    /// Ten minutes at both drift extremes, with a 1 ms control step.
    #[test]
    fn drift_controller() {
        for sign in [-1.0, 1.0] {
            let mut clock = Clock::default();
            let mut level = f64::from(TARGET);
            for _ in 0..600_000 {
                let ratio = clock.ratio(level as u32, 0.001);
                level += 48.0 * (1.0 + sign * 0.0003) - 48.0 / ratio;
                assert!(level > 0.0 && level < f64::from(CAPACITY));
                assert!((0.995..=1.005).contains(&ratio));
            }
            assert!((level - f64::from(TARGET)).abs() < 5.0);
        }
    }

    /// Sparse -29 dBFS spikes, as a silent A16 return carries, never count; a sustained
    /// -48 dBFS tone counts within the third 100 ms window.
    #[test]
    fn level_detection() {
        let mut block = [0.0f32; 80 * 2];
        let mut spikes = Level::default();
        for t in 0..600 * 10u32 {
            block.fill(0.0);
            if t % 80 == 0 {
                block[((t / 80) % 160) as usize] = 0.0355;
            }
            assert!(!spikes.feed(&block, 2));
        }
        let mut tone = Level::default();
        let mut reached = false;
        let mut t = 0u32;
        while t < 600 && !reached {
            for f in 0..80u32 {
                let v = 0.0056
                    * (2.0 * std::f32::consts::PI * 1000.0 * (t * 80 + f) as f32 / 48000.0).sin();
                block[f as usize * 2] = v;
                block[f as usize * 2 + 1] = v;
            }
            reached = tone.feed(&block, 2);
            t += 1;
        }
        assert!(reached && t * 80 >= 3 * LEVEL_FRAMES && t * 80 <= 3 * LEVEL_FRAMES + 80);
    }
}
