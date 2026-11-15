// SPDX-License-Identifier: MIT
//! How the stream has gone since the engine was made: counters the engine keeps on its
//! completion path.

use std::time::Duration;

use crate::PerDirection;

/// One PCM direction's counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DirectionStats {
    /// Completed transfers, failed ones included, aborted ones not.
    pub transfers: u64,
    /// Transfers that completed with an error.
    pub errors: u32,
    /// Transfers that moved less than asked.
    pub short_transfers: u32,
    /// Completions after a gap longer than the queued transfers cover plus 1 ms of jitter:
    /// the device went unserved for a while, the USB side's xrun. Out, it ran dry and
    /// repeated or dropped; in, it overflowed.
    pub late: u32,
    /// The longest gap between two completions of one start.
    pub max_gap: Duration,
}

/// The counters of both directions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub directions: PerDirection<DirectionStats>,
}

impl Stats {
    /// Everything a client hears about ([`Bus::stats_changed`](crate::Bus::stats_changed)):
    /// errors, short and late transfers, both ways.
    pub fn incidents(&self) -> u32 {
        crate::Direction::ALL
            .iter()
            .map(|&d| {
                let s = &self.directions[d];
                s.errors + s.short_transfers + s.late
            })
            .sum()
    }
}
