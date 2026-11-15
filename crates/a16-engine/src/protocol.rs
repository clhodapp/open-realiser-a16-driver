// SPDX-License-Identifier: MIT
//! The Ploytec USB protocol of the A16: control requests, the output packet layout, and the
//! encodings of the sample rate, the firmware version and the input selector byte.
//!
//! Pure computation, no I/O.

use std::fmt;

/// `bmRequestType` of a vendor request from the device ('V', 'I' reads).
pub const REQTYPE_VENDOR_IN: u8 = 0xC0;
/// `bmRequestType` of a vendor request to the device ('I' writes).
pub const REQTYPE_VENDOR_OUT: u8 = 0x40;
/// Vendor request 'V': the firmware version (15 bytes asked, 3 used).
pub const REQUEST_FIRMWARE: u8 = 0x56;
/// Vendor request 'I': a hardware register, by `wIndex`; reads return one byte, writes put
/// the value in `wValue`.
pub const REQUEST_STATUS: u8 = 0x49;
/// `bmRequestType` of the UAC1 endpoint SET_CUR of the sample rate.
pub const REQTYPE_SET_RATE: u8 = 0x22;
/// `bRequest` of SET_CUR.
pub const REQUEST_SET_RATE: u8 = 0x01;
/// `bmRequestType` of the UAC1 GET_CUR of the sample rate.
pub const REQTYPE_GET_RATE: u8 = 0xA2;
/// `bRequest` of GET_CUR.
pub const REQUEST_GET_RATE: u8 = 0x81;
/// `wValue` of the rate requests: the sampling frequency control.
pub const RATE_CONTROL: u16 = 0x0100;
/// `wIndex` of the 'I' register that holds the input selector.
pub const REGISTER_INPUT_SELECTOR: u16 = 0;

/// Input selector bit 1: the clock source is automatic (S/PDIF in when valid), not internal.
pub const SELECTOR_AUTOMATIC_CLOCK: u8 = 0x02;
/// Input selector bit 2: the digital input is locked.
pub const SELECTOR_DIGITAL_LOCK: u8 = 0x04;
/// Input selector bit 4: streaming armed.
pub const SELECTOR_STREAMING_ARMED: u8 = 0x10;
/// Input selector bit 5: 16-lane input DMA.
pub const SELECTOR_INPUT_DMA_16: u8 = 0x20;

/// The MIDI slot of an output sub-packet without MIDI data.
pub const MIDI_IDLE: u8 = 0xFD;
/// The control byte after each MIDI slot, without control data.
pub const CONTROL_IDLE: u8 = 0xFF;

/// Frames in one output sub-packet.
pub const FRAMES_PER_OUT_SUBPACKET: u32 = 10;
/// Frames in one 512-byte input sub-packet.
pub const FRAMES_PER_IN_SUBPACKET: u32 = 8;
/// The most output sub-packets in a transfer (200 frames).
pub const MAX_OUT_SUBPACKETS: u32 = 20;

/// Bytes of an output frame on the wire.
const OUT_FRAME_BYTES: usize = 48;
const BULK_SUBPACKET_BYTES: usize = 512;
const INTERRUPT_SUBPACKET_BYTES: usize = 482;

/// The layout of the output sub-packets of 10 frames, each with a MIDI byte and a control
/// byte. Which one a device uses follows the PCM out endpoint's `wMaxPacketSize`, not its
/// transfer type, like the official driver.
///
/// - Bulk (`wMaxPacketSize` <= 512), 512 bytes per sub-packet:
///   `[480 PCM (10 frames)][MIDI][control][30 pad]`
/// - Interrupt (`wMaxPacketSize` 964), 482 bytes per sub-packet, 2 per USB packet:
///   `[432 PCM (9 frames)][MIDI][control][48 PCM (1 frame)]`
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum OutLayout {
    Bulk,
    #[default]
    Interrupt,
}

impl OutLayout {
    /// The layout for a PCM out endpoint of `max_packet_size` bytes per interval.
    pub const fn for_max_packet(max_packet_size: u16) -> OutLayout {
        if max_packet_size as usize <= BULK_SUBPACKET_BYTES {
            OutLayout::Bulk
        } else {
            OutLayout::Interrupt
        }
    }

    /// Bytes of one sub-packet.
    pub const fn subpacket_bytes(self) -> usize {
        match self {
            OutLayout::Bulk => BULK_SUBPACKET_BYTES,
            OutLayout::Interrupt => INTERRUPT_SUBPACKET_BYTES,
        }
    }

    /// Bytes of a transfer of `frames` frames (frames / 10 sub-packets).
    pub const fn transfer_bytes(self, frames: u32) -> usize {
        (frames / FRAMES_PER_OUT_SUBPACKET) as usize * self.subpacket_bytes()
    }

    /// Byte offset of frame `frame` of a run of sub-packets from the start of its first.
    pub const fn frame_offset(self, frame: u32) -> usize {
        let subpacket = (frame / FRAMES_PER_OUT_SUBPACKET) as usize;
        let index = (frame % FRAMES_PER_OUT_SUBPACKET) as usize;
        let base = subpacket * self.subpacket_bytes() + index * OUT_FRAME_BYTES;
        match self {
            OutLayout::Bulk => base,
            // Frames 0-8, then the MIDI slot and control byte, then frame 9
            OutLayout::Interrupt if index == FRAMES_PER_OUT_SUBPACKET as usize - 1 => base + 2,
            OutLayout::Interrupt => base,
        }
    }

    /// Byte offset of the MIDI slot of sub-packet `subpacket` of a run; the control byte
    /// follows it.
    pub const fn midi_offset(self, subpacket: u32) -> usize {
        let base = subpacket as usize * self.subpacket_bytes();
        match self {
            OutLayout::Bulk => base + FRAMES_PER_OUT_SUBPACKET as usize * OUT_FRAME_BYTES,
            OutLayout::Interrupt => {
                base + (FRAMES_PER_OUT_SUBPACKET as usize - 1) * OUT_FRAME_BYTES
            }
        }
    }

    /// Fill `packet`, whole sub-packets, with silence, idle MIDI slots and idle control
    /// bytes.
    pub fn init_subpackets(self, packet: &mut [u8]) {
        packet.fill(0);
        let count = packet.len() / self.subpacket_bytes();
        for k in 0..count as u32 {
            let midi = self.midi_offset(k);
            packet[midi] = MIDI_IDLE;
            packet[midi + 1] = CONTROL_IDLE;
        }
    }
}

impl fmt::Display for OutLayout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            OutLayout::Bulk => "bulk",
            OutLayout::Interrupt => "interrupt",
        })
    }
}

/// A sample rate as the rate requests carry it: 3 bytes, little-endian.
pub const fn encode_rate(hz: u32) -> [u8; 3] {
    [hz as u8, (hz >> 8) as u8, (hz >> 16) as u8]
}

/// The sample rate in a GET_CUR answer.
pub const fn decode_rate(bytes: [u8; 3]) -> u32 {
    bytes[0] as u32 | (bytes[1] as u32) << 8 | (bytes[2] as u32) << 16
}

/// The firmware version from the 'V' request: shown as 1.major.minor, as the official tools do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FirmwareVersion {
    /// Byte 0 of the answer.
    pub chip_id: u8,
    /// Byte 2 / 10.
    pub major: u8,
    /// Byte 2 % 10.
    pub minor: u8,
}

impl FirmwareVersion {
    /// The version in the first three bytes of a 'V' answer.
    pub const fn parse(answer: [u8; 3]) -> FirmwareVersion {
        FirmwareVersion {
            chip_id: answer[0],
            major: answer[2] / 10,
            minor: answer[2] % 10,
        }
    }
}

impl fmt::Display for FirmwareVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "1.{}.{}", self.major, self.minor)
    }
}

/// `wValue` of an 'I' write of `byte`: sign-extended to 16 bits, like the official driver's
/// `(short)(char)` cast.
pub const fn register_wvalue(byte: u8) -> u16 {
    byte as i8 as i16 as u16
}

/// `wValue` of the status confirm at every stream start: the selector byte with bits 5
/// (16-lane input DMA) and 4 (streaming armed) set, as the official setAJDMAInputChannels
/// writes it.
pub const fn confirm_wvalue(selector: u8) -> u16 {
    register_wvalue(selector | SELECTOR_INPUT_DMA_16 | SELECTOR_STREAMING_ARMED)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts() {
        assert_eq!(OutLayout::for_max_packet(512), OutLayout::Bulk);
        assert_eq!(OutLayout::for_max_packet(964), OutLayout::Interrupt);
        assert_eq!(OutLayout::Interrupt.transfer_bytes(80), 3856);
        assert_eq!(OutLayout::Bulk.transfer_bytes(80), 4096);
        assert_eq!(OutLayout::Interrupt.frame_offset(8), 384);
        assert_eq!(OutLayout::Interrupt.frame_offset(9), 434);
        assert_eq!(OutLayout::Interrupt.frame_offset(10), 482);
        assert_eq!(OutLayout::Interrupt.midi_offset(1), 482 + 432);
        assert_eq!(OutLayout::Bulk.midi_offset(1), 512 + 480);
    }

    #[test]
    fn encodings() {
        assert_eq!(encode_rate(48000), [0x80, 0xBB, 0x00]);
        assert_eq!(decode_rate(encode_rate(96000)), 96000);
        assert_eq!(confirm_wvalue(0x02), 0x0032);
        assert_eq!(confirm_wvalue(0x80), 0xFFB0);
        let fw = FirmwareVersion::parse([0x02, 0x01, 0x04]);
        assert_eq!((fw.chip_id, fw.to_string()), (0x02, "1.0.4".to_owned()));
    }
}
