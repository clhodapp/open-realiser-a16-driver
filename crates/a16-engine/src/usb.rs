// SPDX-License-Identifier: MIT
//! USB endpoint descriptors as the platform reads them, and the pipes the engine runs on.

use std::fmt;

/// An endpoint's transfer type: `bmAttributes & 3`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TransferType {
    Control,
    Isochronous,
    Bulk,
    Interrupt,
}

impl TransferType {
    /// The type in an endpoint descriptor's `bmAttributes`.
    pub const fn from_attributes(attributes: u8) -> TransferType {
        match attributes & 3 {
            0 => TransferType::Control,
            1 => TransferType::Isochronous,
            2 => TransferType::Bulk,
            _ => TransferType::Interrupt,
        }
    }
}

impl fmt::Display for TransferType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            TransferType::Control => "control",
            TransferType::Isochronous => "isoc",
            TransferType::Bulk => "bulk",
            TransferType::Interrupt => "interrupt",
        })
    }
}

/// An endpoint of a claimed interface at its current alternate setting, as its descriptor
/// has it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UsbEndpoint {
    /// `bInterfaceNumber` of the interface it is on.
    pub interface: u8,
    /// `bEndpointAddress`, with the direction bit.
    pub address: u8,
    /// `bmAttributes`.
    pub attributes: u8,
    /// `bInterval`.
    pub interval: u8,
    /// `wMaxPacketSize`: bits 0-10 the packet size, bits 11-12 the extra transactions per
    /// microframe (High Speed high bandwidth).
    pub max_packet: u16,
}

impl UsbEndpoint {
    pub const fn transfer_type(&self) -> TransferType {
        TransferType::from_attributes(self.attributes)
    }

    /// Bytes per service interval: the packet size times the transactions per microframe.
    pub const fn bytes_per_interval(&self) -> u16 {
        (self.max_packet & 0x07FF) * (((self.max_packet >> 11) & 3) + 1)
    }

    /// The pipe the engine runs on this endpoint.
    pub const fn pipe(&self) -> Pipe {
        Pipe {
            transfer_type: self.transfer_type(),
            max_packet_size: self.bytes_per_interval(),
            interval: self.interval,
        }
    }
}

/// A PCM endpoint the engine runs, at the fixed address of its
/// [`Direction`](crate::Direction).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pipe {
    pub transfer_type: TransferType,
    /// Bytes per service interval ([`UsbEndpoint::bytes_per_interval`]).
    pub max_packet_size: u16,
    /// `bInterval` as the descriptor has it.
    pub interval: u8,
}

impl Pipe {
    /// What a PCM transfer's frames must be a multiple of on this pipe, for frames of
    /// `frame_bytes` on the wire: the frames in one whole packet, or 0 for any.
    ///
    /// On bulk and interrupt endpoints the device takes packets whole, both ways: in, it fills
    /// 1024-byte packets of 16 frames, and a request for 8 is overrun (babble); out, it reads
    /// every packet as a 964-byte one of 20 frames, and a 10-frame transfer (one 482-byte
    /// half) plays garbage.
    pub const fn packet_frames(&self, frame_bytes: usize) -> u32 {
        match self.transfer_type {
            TransferType::Bulk | TransferType::Interrupt if frame_bytes != 0 => {
                (self.max_packet_size as usize / frame_bytes) as u32
            }
            _ => 0,
        }
    }
}
