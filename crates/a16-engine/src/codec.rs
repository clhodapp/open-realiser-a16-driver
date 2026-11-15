// SPDX-License-Identifier: MIT
//! The Ploytec bit-interleaved frame format, for the A16's channel counts.
//!
//! An output frame is 48 bytes and carries 16 channels: byte `k` of the first 24 holds bit
//! `23 - k` of the odd channels (1, 3, ..., 15), channel `2j + 1` in bit `j`; the second 24
//! bytes hold the even channels the same way. An input frame is 64 bytes, of which the A16
//! fills two lanes: channel 1 in bit 0 of bytes 0x00..0x17 and channel 2 in bit 0 of bytes
//! 0x20..0x37, most significant bit first. Samples on the host side are S24_3LE.

/// Bytes of an output frame on the wire.
pub const OUT_FRAME_BYTES: usize = 48;
/// Bytes of an input frame on the wire.
pub const IN_FRAME_BYTES: usize = 64;
/// Bytes of 16 S24_3LE samples.
pub const OUT_SAMPLES_BYTES: usize = 16 * 3;
/// Bytes of 2 S24_3LE samples.
pub const IN_SAMPLES_BYTES: usize = 2 * 3;

/// Encode 16 S24_3LE samples into an output frame (the official dmaEncode16).
pub fn encode_frame(frame: &mut [u8; OUT_FRAME_BYTES], samples: &[u8; OUT_SAMPLES_BYTES]) {
    for (half, lanes) in frame.as_chunks_mut::<24>().0.iter_mut().enumerate() {
        for (k, out) in lanes.iter_mut().enumerate() {
            let byte = 2 - k / 8;
            let bit = 7 - k % 8;
            let mut value = 0;
            for j in 0..8 {
                let channel = 2 * j + half;
                value |= ((samples[channel * 3 + byte] >> bit) & 1) << j;
            }
            *out = value;
        }
    }
}

/// Decode an input frame into 2 S24_3LE samples (the official dmaDecode16To02).
pub fn decode_frame(samples: &mut [u8; IN_SAMPLES_BYTES], frame: &[u8; IN_FRAME_BYTES]) {
    for (channel, sample) in samples.as_chunks_mut::<3>().0.iter_mut().enumerate() {
        let lane = &frame[channel * 0x20..channel * 0x20 + 24];
        for (b, out) in sample.iter_mut().enumerate() {
            let top = 23 - 8 * b;
            let mut value = 0;
            for i in 0..8 {
                value |= (lane[top - i] & 1) << i;
            }
            *out = value;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every channel and bit on its own, as test_a16.c checks the C encoder.
    #[test]
    fn single_bit_vectors() {
        for channel in 0..16 {
            for bit in 0..24 {
                let mut samples = [0; OUT_SAMPLES_BYTES];
                let mut expected = [0; OUT_FRAME_BYTES];
                samples[channel * 3 + bit / 8] = 1 << (bit % 8);
                expected[(channel % 2) * 24 + 23 - bit] = 1 << (channel / 2);
                let mut frame = [0; OUT_FRAME_BYTES];
                encode_frame(&mut frame, &samples);
                assert_eq!(frame, expected, "channel {channel} bit {bit}");
            }
        }
    }

    #[test]
    fn decode_stereo() {
        let mut frame = [0; IN_FRAME_BYTES];
        for bit in 0..24 {
            frame[bit] = ((0x123456u32 >> (23 - bit)) & 1) as u8;
            frame[32 + bit] = ((0xedcbaau32 >> (23 - bit)) & 1) as u8;
        }
        let mut samples = [0; IN_SAMPLES_BYTES];
        decode_frame(&mut samples, &frame);
        assert_eq!(samples, [0x56, 0x34, 0x12, 0xaa, 0xcb, 0xed]);
    }
}
