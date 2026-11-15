// SPDX-License-Identifier: MIT
//! The USB protocol engine of the Smyth Realiser A16 (USB 0a4a:5a16), a Ploytec device:
//! the vendor handshake, the bit-interleaved PCM format and the transfer stream, on top of a
//! platform's [`Bus`].
//!
//! The engine is plain data driven from one thread: the platform holds the USB handle and
//! the [`Bus`] around it, calls [`Engine::start`], hands every completed [`Transfer`] to
//! [`Engine::complete`], and on that thread moves PCM between the platform's audio queues
//! and the engine's [`PcmRing`]s.
//!
//! The protocol was reverse-engineered by the Ozzy project, whose C engine this crate ports.

#![forbid(unsafe_code)]

mod bus;
pub mod codec;
mod device;
mod engine;
mod handshake;
pub mod protocol;
mod ring;
mod settings;
mod stats;
mod usb;

pub use bus::{Bus, BusError, ControlSetup, Status, Transfer, TransferId};
pub use device::{
    Direction, INTERFACES, InterfaceSetting, PRODUCT_ID, PRODUCT_NAME, PerDirection, SampleRate,
    VENDOR_ID,
};
pub use engine::{Engine, EngineState, Logger, StartError};
pub use handshake::{HandshakeError, HandshakeInfo};
pub use protocol::{FirmwareVersion, OutLayout};
pub use ring::PcmRing;
pub use settings::{Setting, SettingChange, SettingError, Settings, pcm_frames, pcm_queue};
pub use stats::{DirectionStats, Stats};
pub use usb::{Pipe, TransferType, UsbEndpoint};
