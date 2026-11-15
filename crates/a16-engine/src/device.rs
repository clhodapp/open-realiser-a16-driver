// SPDX-License-Identifier: MIT
//! What the engine knows about the Realiser A16 itself: its USB IDs, interfaces, sample rates
//! and the two PCM directions.
//!
//! The A16 presents two interfaces at alternate setting 1: interface 0 has the control
//! requests and PCM out on endpoint 0x05, interface 1 has PCM in on endpoint 0x86.

use std::fmt;
use std::ops::{Index, IndexMut};

/// The A16's USB vendor ID, which belongs to Ploytec GmbH.
pub const VENDOR_ID: u16 = 0x0A4A;
/// The A16's USB product ID.
pub const PRODUCT_ID: u16 = 0x5A16;
/// The device's product name.
pub const PRODUCT_NAME: &str = "Smyth Research Realiser A16";

/// An interface the platform claims, at the alternate setting it selects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InterfaceSetting {
    /// `bInterfaceNumber`.
    pub number: u8,
    /// `bAlternateSetting` to select after claiming it.
    pub alt_setting: u8,
}

/// The interfaces to claim before streaming, in order.
pub const INTERFACES: [InterfaceSetting; 2] = [
    InterfaceSetting {
        number: 0,
        alt_setting: 1,
    },
    InterfaceSetting {
        number: 1,
        alt_setting: 1,
    },
];

/// A PCM direction: out is playback to the device, in is the device's return.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Direction {
    Out,
    In,
}

impl Direction {
    /// Both directions, out first.
    pub const ALL: [Direction; 2] = [Direction::Out, Direction::In];

    /// The endpoint address, with its direction bit, that carries this direction's PCM.
    pub const fn endpoint(self) -> u8 {
        match self {
            Direction::Out => 0x05,
            Direction::In => 0x86,
        }
    }

    /// The interface the endpoint is on.
    pub const fn interface(self) -> u8 {
        match self {
            Direction::Out => 0,
            Direction::In => 1,
        }
    }

    /// Audio channels the A16 takes (out) or returns (in).
    pub const fn channels(self) -> usize {
        match self {
            Direction::Out => 16,
            Direction::In => 2,
        }
    }

    /// Bytes one frame takes on the wire, in the device's bit-interleaved format.
    pub const fn wire_frame_bytes(self) -> usize {
        match self {
            Direction::Out => 48,
            Direction::In => 64,
        }
    }

    /// Bytes one frame takes in a [`PcmRing`](crate::PcmRing): 24-bit samples, S24_3LE.
    pub const fn ring_frame_bytes(self) -> usize {
        self.channels() * 3
    }

    const fn index(self) -> usize {
        match self {
            Direction::Out => 0,
            Direction::In => 1,
        }
    }
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Direction::Out => "out",
            Direction::In => "in",
        })
    }
}

/// One value per [`Direction`], indexed by it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PerDirection<T>([T; 2]);

impl<T> PerDirection<T> {
    pub const fn new(out: T, input: T) -> Self {
        PerDirection([out, input])
    }

    /// Apply `f` to both values.
    pub fn map<U>(self, mut f: impl FnMut(Direction, T) -> U) -> PerDirection<U> {
        let [out, input] = self.0;
        PerDirection([f(Direction::Out, out), f(Direction::In, input)])
    }
}

impl<T> Index<Direction> for PerDirection<T> {
    type Output = T;

    fn index(&self, direction: Direction) -> &T {
        &self.0[direction.index()]
    }
}

impl<T> IndexMut<Direction> for PerDirection<T> {
    fn index_mut(&mut self, direction: Direction) -> &mut T {
        &mut self.0[direction.index()]
    }
}

/// A sample rate the A16 runs at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SampleRate {
    Hz44100,
    /// What the engine asks for without a request: the A16's SVS processing rate.
    #[default]
    Hz48000,
    Hz88200,
    Hz96000,
}

impl SampleRate {
    /// Every rate the A16 lists, ascending.
    pub const ALL: [SampleRate; 4] = [
        SampleRate::Hz44100,
        SampleRate::Hz48000,
        SampleRate::Hz88200,
        SampleRate::Hz96000,
    ];

    pub const fn hz(self) -> u32 {
        match self {
            SampleRate::Hz44100 => 44100,
            SampleRate::Hz48000 => 48000,
            SampleRate::Hz88200 => 88200,
            SampleRate::Hz96000 => 96000,
        }
    }

    /// The rate of `hz`, if the A16 lists it.
    pub fn from_hz(hz: u32) -> Option<SampleRate> {
        SampleRate::ALL.into_iter().find(|rate| rate.hz() == hz)
    }
}

impl fmt::Display for SampleRate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} Hz", self.hz())
    }
}
