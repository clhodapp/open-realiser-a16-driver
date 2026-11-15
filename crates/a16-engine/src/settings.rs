// SPDX-License-Identifier: MIT
//! The PCM transfer settings: how large the transfers are and how many are in flight, each
//! way. The engine takes them at its next start, so a change restarts streaming
//! ([`Engine::restart`](crate::Engine::restart)).
//!
//! A setting's value is an index into its list of values, as a user interface offers them.

use std::fmt;
use std::ops::Index;

use crate::Direction;

/// One of the engine's settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Setting {
    /// Frames per PCM out transfer: value + 1 sub-packets of 10 frames, 10 to 200.
    PcmOutFrames,
    /// Frames per PCM in transfer: value + 1 sub-packets of 8 frames, 8 to 200.
    PcmInFrames,
    /// PCM out transfers in flight: value + 1, 2 to 4.
    PcmOutQueue,
    /// PCM in transfers in flight: value + 1, 2 to 4.
    PcmInQueue,
}

/// Values of [`Setting::PcmOutFrames`].
const OUT_FRAMES_VALUES: u8 = 20;
/// Values of [`Setting::PcmInFrames`].
const IN_FRAMES_VALUES: u8 = 25;
/// Values of the queue settings. Value 0, one in flight, is listed but never offered.
const QUEUE_VALUES: u8 = 4;
/// The lowest queue value offered: two in flight.
const QUEUE_MIN: u8 = 1;

impl Setting {
    /// Every setting, in the order [`Settings`] keeps them.
    pub const ALL: [Setting; 4] = [
        Setting::PcmOutFrames,
        Setting::PcmInFrames,
        Setting::PcmOutQueue,
        Setting::PcmInQueue,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Setting::PcmOutFrames => "PCM Playback Transfer",
            Setting::PcmInFrames => "PCM Capture Transfer",
            Setting::PcmOutQueue => "PCM Playback Queue",
            Setting::PcmInQueue => "PCM Capture Queue",
        }
    }

    /// How many values the setting has: valid values are `0..value_count()`.
    pub const fn value_count(self) -> u8 {
        match self {
            Setting::PcmOutFrames => OUT_FRAMES_VALUES,
            Setting::PcmInFrames => IN_FRAMES_VALUES,
            Setting::PcmOutQueue | Setting::PcmInQueue => QUEUE_VALUES,
        }
    }

    /// The value a new engine starts with: 80 frames and 4 in flight, each way.
    pub const fn default_value(self) -> u8 {
        match self {
            Setting::PcmOutFrames => 7,
            Setting::PcmInFrames => 9,
            Setting::PcmOutQueue | Setting::PcmInQueue => QUEUE_VALUES - 1,
        }
    }

    /// What `value` means, for a user interface ("80 frames", "4 transfers").
    pub fn value_name(self, value: u8) -> Option<String> {
        if value >= self.value_count() {
            return None;
        }
        Some(match self {
            Setting::PcmOutFrames => format!("{} frames", pcm_frames(Direction::Out, value)),
            Setting::PcmInFrames => format!("{} frames", pcm_frames(Direction::In, value)),
            Setting::PcmOutQueue | Setting::PcmInQueue if value == 0 => "1 transfer".to_owned(),
            Setting::PcmOutQueue | Setting::PcmInQueue => format!("{} transfers", value + 1),
        })
    }

    /// The transfer-size setting of `direction`.
    pub const fn frames(direction: Direction) -> Setting {
        match direction {
            Direction::Out => Setting::PcmOutFrames,
            Direction::In => Setting::PcmInFrames,
        }
    }

    /// The queue setting of `direction`.
    pub const fn queue(direction: Direction) -> Setting {
        match direction {
            Direction::Out => Setting::PcmOutQueue,
            Direction::In => Setting::PcmInQueue,
        }
    }

    const fn index(self) -> usize {
        match self {
            Setting::PcmOutFrames => 0,
            Setting::PcmInFrames => 1,
            Setting::PcmOutQueue => 2,
            Setting::PcmInQueue => 3,
        }
    }
}

impl fmt::Display for Setting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A value for every [`Setting`], indexed by it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings([u8; 4]);

impl Settings {
    pub const fn get(&self, setting: Setting) -> u8 {
        self.0[setting.index()]
    }

    pub(crate) fn set(&mut self, setting: Setting, value: u8) {
        self.0[setting.index()] = value;
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings(Setting::ALL.map(Setting::default_value))
    }
}

impl Index<Setting> for Settings {
    type Output = u8;

    fn index(&self, setting: Setting) -> &u8 {
        &self.0[setting.index()]
    }
}

/// What a setting or rate change comes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingChange {
    /// The value it already has.
    Unchanged,
    /// A new value: [`Engine::restart`](crate::Engine::restart) applies it.
    NeedsRestart,
}

/// Why the engine refused a setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingError {
    /// The setting has no such value.
    OutOfRange { setting: Setting, value: u8 },
    /// A transfer size that isn't whole packets of the endpoint.
    NotWholePackets {
        setting: Setting,
        frames: u32,
        packet_frames: u32,
    },
    /// One transfer in flight, which is never offered.
    QueueTooShort { setting: Setting },
}

impl fmt::Display for SettingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            SettingError::OutOfRange { setting, value } => write!(f, "{setting}: no value {value}"),
            SettingError::NotWholePackets {
                setting,
                frames,
                packet_frames,
            } => write!(
                f,
                "{setting}: {frames} frames aren't whole packets of {packet_frames} frames"
            ),
            SettingError::QueueTooShort { setting } => {
                write!(f, "{setting}: one transfer in flight isn't offered")
            }
        }
    }
}

impl std::error::Error for SettingError {}

/// Frames per transfer of frame-setting value `value` in `direction`; a value out of range
/// gives the default.
pub fn pcm_frames(direction: Direction, value: u8) -> u32 {
    let setting = Setting::frames(direction);
    let value = if value < setting.value_count() {
        value
    } else {
        setting.default_value()
    };
    let per_subpacket = match direction {
        Direction::Out => 10,
        Direction::In => 8,
    };
    (u32::from(value) + 1) * per_subpacket
}

/// Transfers in flight of queue-setting value `value`: value 0 runs as two, a value out of
/// range as the most.
pub fn pcm_queue(value: u8) -> u32 {
    u32::from(value.clamp(QUEUE_MIN, QUEUE_VALUES - 1)) + 1
}

/// Checks `value` for `setting` on a device whose transfers in `direction` must be multiples
/// of `packet_frames(direction)` frames (0: any).
pub(crate) fn check_value(
    setting: Setting,
    value: u8,
    packet_frames: impl Fn(Direction) -> u32,
) -> Result<(), SettingError> {
    if value >= setting.value_count() {
        return Err(SettingError::OutOfRange { setting, value });
    }
    let direction = match setting {
        Setting::PcmOutQueue | Setting::PcmInQueue if value < QUEUE_MIN => {
            return Err(SettingError::QueueTooShort { setting });
        }
        Setting::PcmOutQueue | Setting::PcmInQueue => return Ok(()),
        Setting::PcmOutFrames => Direction::Out,
        Setting::PcmInFrames => Direction::In,
    };
    let frames = pcm_frames(direction, value);
    let step = packet_frames(direction);
    if step != 0 && !frames.is_multiple_of(step) {
        return Err(SettingError::NotWholePackets {
            setting,
            frames,
            packet_frames: step,
        });
    }
    Ok(())
}

/// The largest transfer in frames, both ways.
pub(crate) const MAX_TRANSFER_FRAMES: u32 = 200;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_and_queues() {
        assert_eq!(pcm_frames(Direction::Out, 7), 80);
        assert_eq!(pcm_frames(Direction::In, 9), 80);
        assert_eq!(pcm_frames(Direction::Out, 19), 200);
        assert_eq!(pcm_frames(Direction::In, 24), 200);
        assert_eq!(pcm_frames(Direction::Out, 20), 80);
        assert_eq!(pcm_queue(0), 2);
        assert_eq!(pcm_queue(1), 2);
        assert_eq!(pcm_queue(3), 4);
        assert_eq!(pcm_queue(9), 4);
    }

    #[test]
    fn value_names() {
        assert_eq!(
            Setting::PcmOutFrames.value_name(0).as_deref(),
            Some("10 frames")
        );
        assert_eq!(
            Setting::PcmInFrames.value_name(24).as_deref(),
            Some("200 frames")
        );
        assert_eq!(
            Setting::PcmInQueue.value_name(0).as_deref(),
            Some("1 transfer")
        );
        assert_eq!(
            Setting::PcmOutQueue.value_name(3).as_deref(),
            Some("4 transfers")
        );
        assert_eq!(Setting::PcmOutQueue.value_name(4), None);
    }
}
