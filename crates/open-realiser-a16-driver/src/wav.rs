// SPDX-License-Identifier: MIT
//! WAV files of S24_3LE PCM, with an explicit rate and channel count, and no conversion.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

fn le16(p: &[u8]) -> u16 {
    u16::from_le_bytes([p[0], p[1]])
}

fn le32(p: &[u8]) -> u32 {
    u32::from_le_bytes([p[0], p[1], p[2], p[3]])
}

/// The PCM of the WAV file at `path` into `dst`, which holds `dst.len() / (channels * 3)`
/// frames; the frames read. None unless the file is S24_3LE PCM at `rate` with `channels`
/// channels and fits.
pub fn read(path: &Path, rate: u32, channels: u16, dst: &mut [u8]) -> Option<u32> {
    let mut f = File::open(path).ok()?;
    read_from(&mut f, rate, channels, dst)
}

fn read_from(f: &mut File, rate: u32, channels: u16, dst: &mut [u8]) -> Option<u32> {
    let mut h = [0u8; 40];
    f.read_exact(&mut h[..12]).ok()?;
    if &h[..4] != b"RIFF" || &h[8..12] != b"WAVE" {
        return None;
    }
    let mut left = u64::from(le32(&h[4..]));
    if left < 4 {
        return None;
    }
    left -= 4;
    let mut format = false;
    let width = u32::from(channels) * 3;
    while left >= 8 {
        f.read_exact(&mut h[..8]).ok()?;
        let n = le32(&h[4..]);
        left -= 8;
        let padded = u64::from(n) + u64::from(n & 1);
        if padded > left {
            return None;
        }
        let id = [h[0], h[1], h[2], h[3]];
        if &id == b"fmt " {
            if format || n < 16 || n as usize > h.len() {
                return None;
            }
            f.read_exact(&mut h[..n as usize]).ok()?;
            let mut pcm = le16(&h) == 1;
            // WAVE_FORMAT_EXTENSIBLE with the PCM subtype and 24 valid bits
            if le16(&h) == 0xfffe && n == 40 {
                const PCM_GUID: [u8; 16] = [
                    1, 0, 0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71,
                ];
                pcm = le16(&h[16..]) == 22 && le16(&h[18..]) == 24 && h[24..40] == PCM_GUID;
            }
            if !pcm
                || channels == 0
                || le16(&h[2..]) != channels
                || le32(&h[4..]) != rate
                || le16(&h[14..]) != 24
                || u32::from(le16(&h[12..])) != width
                || le32(&h[8..]) != rate.wrapping_mul(width)
            {
                return None;
            }
            format = true;
        } else if &id == b"data" {
            let capacity = dst.len() as u32 / width.max(1);
            if !format || width == 0 || !n.is_multiple_of(width) || n / width > capacity {
                return None;
            }
            f.read_exact(&mut dst[..n as usize]).ok()?;
            return Some(n / width);
        } else {
            f.seek(SeekFrom::Current(i64::from(n))).ok()?;
        }
        if n & 1 != 0 {
            let mut pad = [0u8];
            f.read_exact(&mut pad).ok()?;
        }
        left -= padded;
    }
    None
}

/// `frames` frames of `src` as a WAV file of S24_3LE PCM, into `f`, which the caller created
/// new so that no recording is overwritten.
pub fn write(f: &mut File, rate: u32, channels: u16, src: &[u8], frames: u32) -> io::Result<()> {
    let size = u64::from(frames) * u64::from(channels) * 3;
    if channels == 0 || size > u64::from(u32::MAX - 36) {
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    }
    let width = u32::from(channels) * 3;
    let mut h = Vec::with_capacity(44);
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&(size as u32 + 36).to_le_bytes());
    h.extend_from_slice(b"WAVEfmt ");
    h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&1u16.to_le_bytes());
    h.extend_from_slice(&channels.to_le_bytes());
    h.extend_from_slice(&rate.to_le_bytes());
    h.extend_from_slice(&rate.wrapping_mul(width).to_le_bytes());
    h.extend_from_slice(&(width as u16).to_le_bytes());
    h.extend_from_slice(&24u16.to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&(size as u32).to_le_bytes());
    f.write_all(&h)?;
    f.write_all(&src[..size as usize])?;
    f.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation() {
        const SAMPLE: [u8; 6] = [0x56, 0x34, 0x12, 0xaa, 0xcb, 0xed];
        let dir = std::env::temp_dir().join(format!("a16-wav-{}", std::process::id()));
        let _ = std::fs::remove_file(&dir);
        let mut f = File::create_new(&dir).expect("temporary WAV");
        write(&mut f, 44100, 2, &SAMPLE, 1).expect("write");
        drop(f);
        let mut dst = [0u8; 6];
        assert_eq!(read(&dir, 44100, 2, &mut dst), Some(1));
        assert_eq!(dst, SAMPLE);
        assert_eq!(read(&dir, 48000, 2, &mut dst), None);
        assert_eq!(read(&dir, 44100, 16, &mut [0u8; 48]), None);
        assert_eq!(read(&dir, 44100, 2, &mut []), None);
        std::fs::write(&dir, b"RIFF").expect("truncate");
        assert_eq!(read(&dir, 44100, 2, &mut dst), None);
        std::fs::remove_file(&dir).expect("remove");
    }
}
