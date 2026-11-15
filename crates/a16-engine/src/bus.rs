// SPDX-License-Identifier: MIT
//! The platform's USB services: transfers, control requests, a clock, and what the engine
//! tells the platform on its completion path.

use std::fmt;
use std::time::Duration;

use crate::{Direction, PcmRing};

/// Identifies a PCM transfer: a sequence number per direction, counted from 0 at each start.
/// The transfer after one with id `n` completes is queued as `n + in_flight`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TransferId(pub u32);

impl TransferId {
    pub(crate) const fn after(self, n: u32) -> TransferId {
        TransferId(self.0.wrapping_add(n))
    }
}

impl fmt::Display for TransferId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A PCM transfer and its buffer. The engine fills it and hands it to [`Bus::submit`]; the
/// platform keeps it while it is on the bus, then gives it back with
/// [`Engine::complete`](crate::Engine::complete), or from [`Bus::abort`].
///
/// An out transfer goes to the device as it is. An in transfer comes back with the bytes the
/// device sent at the start of its buffer.
#[derive(Debug)]
pub struct Transfer {
    direction: Direction,
    id: TransferId,
    buffer: Vec<u8>,
}

impl Transfer {
    pub(crate) fn new(direction: Direction, id: TransferId, buffer: Vec<u8>) -> Transfer {
        Transfer {
            direction,
            id,
            buffer,
        }
    }

    pub const fn direction(&self) -> Direction {
        self.direction
    }

    /// The endpoint address it goes to, with the direction bit.
    pub const fn endpoint(&self) -> u8 {
        self.direction.endpoint()
    }

    pub const fn id(&self) -> TransferId {
        self.id
    }

    pub(crate) fn set_id(&mut self, id: TransferId) {
        self.id = id;
    }

    /// Bytes to send or to receive: the buffer's length.
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    pub fn buffer(&self) -> &[u8] {
        &self.buffer
    }

    /// The buffer, for the platform to receive into. Its length is fixed.
    pub fn buffer_mut(&mut self) -> &mut [u8] {
        &mut self.buffer
    }
}

/// How a transfer ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// It moved this many bytes; fewer than [`Transfer::len`] is a short transfer.
    Completed(usize),
    /// The bus reported an error. Out, the engine queues the next transfer as usual; in, the
    /// transfer's frames are silence.
    Failed,
    /// Cancelled, by [`Bus::abort`] or by the platform: the engine queues nothing after it.
    Aborted,
}

/// A control request's setup packet, without `wLength` (the data's length).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControlSetup {
    pub request_type: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
}

/// A platform error, with the error code the platform reports (a libusb error code, for a
/// libusb bus).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BusError(pub i32);

impl fmt::Display for BusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "USB error {}", self.0)
    }
}

impl std::error::Error for BusError {}

/// The platform's services. The engine calls them only from inside its methods, so a
/// platform that calls the engine from one thread gets every call on that thread.
///
/// The synchronous calls (`control_in`, `control_out`, `clear_halt`, `sleep`, `abort`) come
/// only from [`Engine::start`](crate::Engine::start), [`Engine::stop`](crate::Engine::stop),
/// [`Engine::suspend`](crate::Engine::suspend) and [`Engine::restart`](crate::Engine::restart),
/// never from [`Engine::complete`](crate::Engine::complete).
pub trait Bus {
    /// Queue `transfer` on its endpoint, a bulk or interrupt one. On failure it comes back and
    /// the engine drops it: one fewer transfer is in flight.
    fn submit(&mut self, transfer: Transfer) -> Result<(), Transfer>;

    /// A control request that reads `data.len()` bytes into `data` (zeroed by the engine).
    fn control_in(&mut self, setup: ControlSetup, data: &mut [u8]) -> Result<(), BusError>;

    /// A control request that writes `data` (empty: no data stage).
    fn control_out(&mut self, setup: ControlSetup, data: &[u8]) -> Result<(), BusError>;

    /// Clear ENDPOINT_HALT on `endpoint` (with the direction bit) and reset the host side of
    /// its pipe.
    fn clear_halt(&mut self, endpoint: u8) -> Result<(), BusError>;

    fn sleep(&mut self, duration: Duration);

    /// Cancel every transfer in flight and return them once the bus no longer uses their
    /// buffers. The pipes stay usable.
    fn abort(&mut self) -> Vec<Transfer>;

    /// The engine moved PCM through the ring attached for `direction`: `position` is the ring
    /// position after the transfer, and `ring` the ring itself. Out, the ring's frames up to
    /// `position` have been encoded, so the platform may write the next ones; in, the frames
    /// up to `position` have been decoded into it.
    fn pcm_progress(&mut self, direction: Direction, position: u64, ring: &mut PcmRing) {
        let _ = (direction, position, ring);
    }
}
