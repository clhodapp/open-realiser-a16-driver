// SPDX-License-Identifier: MIT
//! The PCM rings the engine encodes from and decodes into.

use crate::Direction;

/// An interleaved S24_3LE ring of one [`Direction`]'s channels (16 out, 2 in), owned by the
/// engine while it is attached ([`Engine::attach_pcm_ring`](crate::Engine::attach_pcm_ring)).
///
/// The engine reads or writes a transfer's frames at the ring position it keeps, wrapping
/// at the ring's end, and passes the ring to
/// [`Bus::pcm_progress`](crate::Bus::pcm_progress) after each transfer. Between
/// completions the platform reaches it through
/// [`Engine::pcm_ring_mut`](crate::Engine::pcm_ring_mut).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PcmRing {
    direction: Direction,
    frames: usize,
    data: Vec<u8>,
}

impl PcmRing {
    /// A silent ring of `frames` frames.
    ///
    /// # Panics
    ///
    /// If `frames` is 0.
    pub fn new(direction: Direction, frames: usize) -> PcmRing {
        PcmRing::from_bytes(direction, vec![0; frames * direction.ring_frame_bytes()])
    }

    /// A ring holding `data`, whole frames.
    ///
    /// # Panics
    ///
    /// If `data` is empty or not whole frames.
    pub fn from_bytes(direction: Direction, data: Vec<u8>) -> PcmRing {
        let frame_bytes = direction.ring_frame_bytes();
        assert!(
            !data.is_empty() && data.len().is_multiple_of(frame_bytes),
            "a PCM ring holds one or more whole frames of {frame_bytes} bytes"
        );
        PcmRing {
            direction,
            frames: data.len() / frame_bytes,
            data,
        }
    }

    pub const fn direction(&self) -> Direction {
        self.direction
    }

    pub const fn frames(&self) -> usize {
        self.frames
    }

    pub const fn channels(&self) -> usize {
        self.direction.channels()
    }

    /// Bytes of one frame.
    pub const fn frame_bytes(&self) -> usize {
        self.direction.ring_frame_bytes()
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    pub fn as_bytes_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }

    /// Frame `index` (of `0..frames()`).
    pub fn frame(&self, index: usize) -> &[u8] {
        let bytes = self.frame_bytes();
        &self.data[index * bytes..(index + 1) * bytes]
    }

    pub fn frame_mut(&mut self, index: usize) -> &mut [u8] {
        let bytes = self.frame_bytes();
        &mut self.data[index * bytes..(index + 1) * bytes]
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.data
    }
}
