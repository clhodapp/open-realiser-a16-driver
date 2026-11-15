// SPDX-License-Identifier: MIT
//! A client for the TCP command server of the Smyth Realiser A16: the commands, the framing
//! of the replies, typed decoding of the reply items, and a connection that matches replies
//! to requests. `docs/development/ip-control.md` describes the protocol.

#![forbid(unsafe_code)]

pub mod command;
pub mod connection;
#[cfg(test)]
mod fixtures;
pub mod parser;
pub mod response;
pub mod speaker;

pub use command::{Command, Input, Key, Power, Preset, Step, User};
pub use connection::{Connection, Endpoint, Packet};
pub use parser::{Event, Item, Parser};
pub use response::{FirmwareField, PowerState, Status, SystemField, Update, UserField, UserStatus};
pub use speaker::{Category, SPEAKERS, Speaker, SpeakerId};
