// SPDX-License-Identifier: MIT
//! Typed decoding of reply items, and [`Status`], the state the items describe.

use std::collections::BTreeMap;

use crate::command::User;
use crate::parser::Item;

/// The value of `PWR`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerState {
    On,
    Standby,
    Boot,
}

/// A text field of the info of a user (`80`, `a0`). The key is the letter of the user and
/// the suffix, such as `AROOM`; the name is `AUR` and `BUR`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum UserField {
    Name,
    Room,
    Subject,
    Layout,
    HpeqFile,
    HpeqSubject,
    HpeqDate,
    HpeqType,
    HpeqModel,
    Tactile,
}

impl UserField {
    const ALL: [(UserField, &'static str, &'static str); 10] = [
        (UserField::Name, "UR", "user"),
        (UserField::Room, "ROOM", "room"),
        (UserField::Subject, "NAME", "subject"),
        (UserField::Layout, "SPKR", "room layout"),
        (UserField::HpeqFile, "QFILE", "HPEQ file"),
        (UserField::HpeqSubject, "QNAME", "HPEQ subject"),
        (UserField::HpeqDate, "QDATE", "HPEQ date"),
        (UserField::HpeqType, "QTYPE", "HPEQ type"),
        (UserField::HpeqModel, "QMOD", "HPEQ headphone"),
        (UserField::Tactile, "TACT", "tactile"),
    ];

    pub fn label(self) -> &'static str {
        UserField::ALL.iter().find(|f| f.0 == self).unwrap().2
    }
}

/// A field that describes the A16 rather than one user. User A info carries them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SystemField {
    Input,
    Decoder,
    ListeningMode,
    Upmix,
    HeadTrackerMode,
    Legacy,
    UserMode,
}

impl SystemField {
    const ALL: [(SystemField, &'static str, &'static str); 7] = [
        (SystemField::Input, "IN", "input"),
        (SystemField::Decoder, "DEC", "decoder"),
        (SystemField::ListeningMode, "LM", "listening mode"),
        (SystemField::Upmix, "UMIX", "upmix"),
        (SystemField::HeadTrackerMode, "HTMODE", "head tracker mode"),
        (SystemField::Legacy, "LEG", "legacy mode"),
        (SystemField::UserMode, "USER", "user mode"),
    ];

    pub fn label(self) -> &'static str {
        SystemField::ALL.iter().find(|f| f.0 == self).unwrap().2
    }
}

/// A field of the firmware reply (`64`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FirmwareField {
    A16,
    Apm,
    Hsr,
    HeadTracker,
    Serial,
}

impl FirmwareField {
    const ALL: [(FirmwareField, &'static str, &'static str); 5] = [
        (FirmwareField::A16, "A16FW", "A16 firmware"),
        (FirmwareField::Apm, "APMFW", "APM firmware"),
        (FirmwareField::Hsr, "HSRFW", "HSR firmware"),
        (FirmwareField::HeadTracker, "HTFW", "head tracker firmware"),
        (FirmwareField::Serial, "A16SN", "serial number"),
    ];

    pub fn label(self) -> &'static str {
        FirmwareField::ALL.iter().find(|f| f.0 == self).unwrap().2
    }
}

/// What one item says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Update {
    Power(PowerState),
    /// `KEY=nn`: the IR key of code `nn` was handled.
    Key(u8),
    /// `PAGE`: the menu page shown, 0 for the home page.
    Page(u16),
    /// `ALL`: `SOLO` or `MUTE`.
    All(String),
    /// `TEST`: `LOOP` or `OFF`.
    Test(String),
    /// `MODE`: `SVS`, `AV` or `MIX`.
    Mode(String),
    /// `HT`: `ACTIVE` or `OFF`.
    HeadTracking(String),
    /// `MUTEA`, `MUTEB`: muted or not.
    Mute(User, bool),
    Preset(User, u8),
    Volume(User, u8),
    /// `TRI`: the tri-level volume, 1 to 3.
    Tri(u8),
    UserText(User, UserField, String),
    System(SystemField, String),
    Firmware(FirmwareField, String),
    /// `ACHn`, `BCHn`: the speaker designator of channel `n`, empty when unassigned.
    Assignment(User, u8, String),
    /// `Ln`, `AHL` and the like: a level meter, 0 to 99.
    Level(String, u8),
    /// `AHT` and its variants: the key suffix after `AHT` (`A`, `E`, or empty) and the value.
    HeadAngle(User, String, String),
    /// `A00`: the list of active speakers follows.
    ActiveListStart,
    /// `Ann`: speaker `nn` is active.
    Active(u8),
    /// `Mnn`: speaker `nn` is muted.
    Muted(u8),
    /// `V00`: the list of visible speaker keys follows.
    VisibleListStart,
    /// `Vnn`: the key of speaker `nn` is shown.
    Visible(u8),
    Unknown(Item),
}

fn user_of(letter: char) -> Option<User> {
    match letter {
        'A' => Some(User::A),
        'B' => Some(User::B),
        _ => None,
    }
}

fn two_digits(text: &str) -> Option<u8> {
    (text.len() == 2 && text.bytes().all(|b| b.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
}

fn is_level_key(key: &str) -> bool {
    let b = key.as_bytes();
    let numbered = b.len() > 1 && b[0] == b'L' && b[1..].iter().all(u8::is_ascii_digit);
    let output = b.len() == 3
        && matches!(b[0], b'A' | b'B')
        && matches!(b[1], b'H' | b'T')
        && matches!(b[2], b'L' | b'R');
    numbered || output
}

impl Update {
    pub fn decode(item: &Item) -> Update {
        Update::decode_known(item).unwrap_or_else(|| Update::Unknown(item.clone()))
    }

    fn decode_known(item: &Item) -> Option<Update> {
        let (key, value) = match item {
            Item::Token(token) => return Update::decode_token(token),
            Item::Field { key, value } => (key.as_str(), value.as_str()),
        };
        let text = || value.to_string();
        let number = || value.trim().parse::<u8>().ok();
        if let Some(&(field, ..)) = SystemField::ALL.iter().find(|f| f.1 == key) {
            return Some(Update::System(field, text()));
        }
        if let Some(&(field, ..)) = FirmwareField::ALL.iter().find(|f| f.1 == key) {
            return Some(Update::Firmware(field, text()));
        }
        let update = match key {
            "PWR" => Update::Power(match value {
                "ON" => PowerState::On,
                "STANDBY" => PowerState::Standby,
                "BOOT" => PowerState::Boot,
                _ => return None,
            }),
            "KEY" => Update::Key(u8::from_str_radix(value, 16).ok()?),
            "PAGE" => Update::Page(value.trim().parse().ok()?),
            "ALL" => Update::All(text()),
            "TEST" => Update::Test(text()),
            "MODE" => Update::Mode(text()),
            "HT" => Update::HeadTracking(text()),
            "MUTEA" | "MUTEB" => Update::Mute(
                user_of(key.chars().last()?)?,
                match value {
                    "MUTE" => true,
                    "OFF" => false,
                    _ => return None,
                },
            ),
            "PA" | "PB" => Update::Preset(user_of(key.chars().nth(1)?)?, number()?),
            "VA" | "VB" => Update::Volume(user_of(key.chars().nth(1)?)?, number()?),
            "TRI" => Update::Tri(number()?),
            _ if is_level_key(key) => Update::Level(key.to_string(), number()?),
            _ => {
                let user = user_of(key.chars().next()?)?;
                let rest = &key[1..];
                if let Some(channel) = rest.strip_prefix("CH") {
                    let channel = channel.parse().ok()?;
                    Update::Assignment(user, channel, value.trim().to_string())
                } else if let Some(axis) = rest.strip_prefix("HT") {
                    Update::HeadAngle(user, axis.to_string(), text())
                } else {
                    let &(field, ..) = UserField::ALL.iter().find(|f| f.1 == rest)?;
                    Update::UserText(user, field, text())
                }
            }
        };
        Some(update)
    }

    fn decode_token(token: &str) -> Option<Update> {
        let (letter, digits) = token.split_at_checked(1)?;
        let n = two_digits(digits)?;
        Some(match (letter, n) {
            ("A", 0) => Update::ActiveListStart,
            ("A", n) => Update::Active(n),
            ("M", n) if n > 0 => Update::Muted(n),
            ("V", 0) => Update::VisibleListStart,
            ("V", n) => Update::Visible(n),
            _ => return None,
        })
    }
}

/// What the replies say about one user.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UserStatus {
    pub text: BTreeMap<UserField, String>,
    pub preset: Option<u8>,
    pub volume: Option<u8>,
    pub mute: Option<bool>,
    /// The speaker designator of each channel, empty for an unassigned channel.
    pub assignments: BTreeMap<u8, String>,
    /// The head-tracker angles by key suffix, `A` and `E` on firmware 2.17.
    pub head_angle: BTreeMap<String, String>,
}

/// The state of the A16 as the items received so far describe it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub power: Option<PowerState>,
    pub firmware: BTreeMap<FirmwareField, String>,
    pub system: BTreeMap<SystemField, String>,
    pub user_a: UserStatus,
    pub user_b: UserStatus,
    pub all: Option<String>,
    pub test: Option<String>,
    pub mode: Option<String>,
    pub head_tracking: Option<String>,
    pub tri: Option<u8>,
    pub page: Option<u16>,
    pub last_key: Option<u8>,
    /// Level meters, in the order they first arrived.
    pub levels: Vec<(String, u8)>,
    /// Speaker IDs, in the order the A16 lists them.
    pub active: Vec<u8>,
    pub muted: Vec<u8>,
    pub visible: Vec<u8>,
    /// Items with no decoding, the latest of each key.
    pub unknown: Vec<Item>,
}

fn insert(list: &mut Vec<u8>, id: u8) {
    if !list.contains(&id) {
        list.push(id);
    }
}

impl Status {
    pub fn user(&self, user: User) -> &UserStatus {
        user.pick(&self.user_a, &self.user_b)
    }

    fn user_mut(&mut self, user: User) -> &mut UserStatus {
        match user {
            User::A => &mut self.user_a,
            User::B => &mut self.user_b,
        }
    }

    pub fn apply(&mut self, item: &Item) {
        match Update::decode(item) {
            Update::Power(state) => self.power = Some(state),
            Update::Key(code) => self.last_key = Some(code),
            Update::Page(page) => self.page = Some(page),
            Update::All(v) => self.all = Some(v),
            Update::Test(v) => self.test = Some(v),
            Update::Mode(v) => self.mode = Some(v),
            Update::HeadTracking(v) => self.head_tracking = Some(v),
            Update::Mute(user, muted) => self.user_mut(user).mute = Some(muted),
            Update::Preset(user, n) => self.user_mut(user).preset = Some(n),
            Update::Volume(user, n) => self.user_mut(user).volume = Some(n),
            Update::Tri(n) => self.tri = Some(n),
            Update::UserText(user, field, v) => {
                self.user_mut(user).text.insert(field, v);
            }
            Update::System(field, v) => {
                self.system.insert(field, v);
            }
            Update::Firmware(field, v) => {
                self.firmware.insert(field, v);
            }
            Update::Assignment(user, channel, v) => {
                self.user_mut(user).assignments.insert(channel, v);
            }
            Update::Level(name, v) => match self.levels.iter_mut().find(|l| l.0 == name) {
                Some(level) => level.1 = v,
                None => self.levels.push((name, v)),
            },
            Update::HeadAngle(user, axis, v) => {
                self.user_mut(user).head_angle.insert(axis, v);
            }
            Update::ActiveListStart => {
                self.active.clear();
                self.muted.clear();
            }
            Update::Active(id) => {
                self.muted.retain(|&m| m != id);
                insert(&mut self.active, id);
            }
            Update::Muted(id) => {
                self.active.retain(|&a| a != id);
                insert(&mut self.muted, id);
            }
            Update::VisibleListStart => self.visible.clear(),
            Update::Visible(id) => insert(&mut self.visible, id),
            Update::Unknown(item) => {
                let same = |known: &Item| item.key().is_some() && known.key() == item.key();
                match self.unknown.iter_mut().find(|known| same(known)) {
                    Some(known) => *known = item,
                    None => self.unknown.push(item),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use crate::parser::{Event, Parser};

    fn status(stream: &[u8]) -> Status {
        let mut status = Status::default();
        for event in Parser::new().feed(stream) {
            if let Event::Item(item) = event {
                status.apply(&item);
            }
        }
        status
    }

    #[test]
    fn decodes_single_items() {
        let d = |s| Update::decode(&Item::parse(s));
        assert_eq!(d("PWR=STANDBY"), Update::Power(PowerState::Standby));
        assert_eq!(d("KEY=5a"), Update::Key(0x5a));
        assert_eq!(d("PAGE=12"), Update::Page(12));
        assert_eq!(d("MUTEB=MUTE"), Update::Mute(User::B, true));
        assert_eq!(d("VB=27"), Update::Volume(User::B, 27));
        assert_eq!(d("PA=03"), Update::Preset(User::A, 3));
        assert_eq!(d("TRI=2"), Update::Tri(2));
        assert_eq!(
            d("IN=HDMI-1"),
            Update::System(SystemField::Input, "HDMI-1".into())
        );
        assert_eq!(d("L3=0 "), Update::Level("L3".into(), 0));
        assert_eq!(d("BTR=99"), Update::Level("BTR".into(), 99));
        assert_eq!(d("ACH17= "), Update::Assignment(User::A, 17, String::new()));
        assert_eq!(d("BCH2=R"), Update::Assignment(User::B, 2, "R".into()));
        assert_eq!(
            d("AHT=12"),
            Update::HeadAngle(User::A, String::new(), "12".into())
        );
        assert_eq!(
            d("BHTE=0"),
            Update::HeadAngle(User::B, "E".into(), "0".into())
        );
        assert_eq!(d("A00"), Update::ActiveListStart);
        assert_eq!(d("A24"), Update::Active(24));
        assert_eq!(d("M03"), Update::Muted(3));
        assert_eq!(d("V17"), Update::Visible(17));
        assert_eq!(
            d("BQDATE=17:35"),
            Update::UserText(User::B, UserField::HpeqDate, "17:35".into())
        );
        for unknown in [
            "PWR=MAYBE",
            "VA=loud",
            "XYZ=1",
            "A1",
            "M00",
            "Q01",
            "ACHx=L",
        ] {
            assert_eq!(
                d(unknown),
                Update::Unknown(Item::parse(unknown)),
                "{unknown}"
            );
        }
    }

    #[test]
    fn decodes_firmware() {
        let s = status(fixtures::FIRMWARE);
        assert_eq!(s.firmware[&FirmwareField::A16], "2.17 Jan 26 2024");
        assert_eq!(s.firmware[&FirmwareField::Apm], "APM 117 v251, Aug  6 2021");
        assert_eq!(s.firmware[&FirmwareField::Hsr], "v74.50.38");
        assert_eq!(s.firmware[&FirmwareField::HeadTracker], "1.40 May 21 2021");
        assert_eq!(s.firmware[&FirmwareField::Serial], "A162U2001#000304");
        assert!(s.unknown.is_empty());
    }

    #[test]
    fn decodes_user_info() {
        let s = status(&[fixtures::USER_A_INFO, fixtures::USER_B_INFO].concat());
        assert_eq!(s.user_a.text[&UserField::Name], "User 1");
        assert_eq!(s.user_a.preset, Some(1));
        assert_eq!(s.user_a.volume, Some(50));
        assert_eq!(s.user_a.text[&UserField::Room], "3DSoundShop");
        assert_eq!(s.user_a.text[&UserField::Subject], "Omega_96_Pro");
        assert_eq!(s.user_a.text[&UserField::Layout], "");
        assert_eq!(s.user_a.text[&UserField::HpeqFile], "Chris  _Focal Clear ");
        assert_eq!(s.user_a.text[&UserField::HpeqSubject], "Chris  ");
        assert_eq!(s.user_a.text[&UserField::HpeqDate], "17:35 20/06/2022");
        assert_eq!(s.user_a.text[&UserField::HpeqType], "autoEQ");
        assert_eq!(s.user_a.text[&UserField::HpeqModel], "Focal Clear ");
        assert_eq!(s.user_a.text[&UserField::Tactile], "");
        assert_eq!(s.system[&SystemField::Input], "USB");
        assert_eq!(s.system[&SystemField::Decoder], "PCM 16ch");
        assert_eq!(s.system[&SystemField::ListeningMode], "16.0");
        assert_eq!(s.system[&SystemField::Upmix], "");
        assert_eq!(s.system[&SystemField::HeadTrackerMode], "Gyro");
        assert_eq!(s.system[&SystemField::Legacy], "");
        assert_eq!(s.system[&SystemField::UserMode], "DUAL");
        assert_eq!(s.user_b.text[&UserField::Name], "User 2");
        assert_eq!(s.user_b.preset, Some(2));
        assert_eq!(s.user_b.text.len(), 10);
        assert!(s.unknown.is_empty());
    }

    #[test]
    fn keeps_unknown_items() {
        let s = status(b"AUR=User 1\0AXYZ=1\0NEW=a\0NEW=b\0Q7\0\r");
        assert_eq!(
            s.unknown,
            [
                Item::parse("AXYZ=1"),
                Item::parse("NEW=b"),
                Item::parse("Q7")
            ]
        );
    }

    #[test]
    fn decodes_assignments() {
        let s = status(fixtures::ASSIGNMENTS);
        assert_eq!(s.user_a.assignments.len(), 24);
        assert_eq!(s.user_a.assignments[&1], "L");
        assert_eq!(s.user_a.assignments[&16], "T");
        assert_eq!(s.user_a.assignments[&17], "");
        assert_eq!(s.user_b.assignments.len(), 16);
        assert_eq!(s.user_b.assignments[&5], "Lc");
    }

    #[test]
    fn decodes_volumes_angles_and_levels() {
        let s = status(
            &[
                fixtures::VOLUME_A,
                fixtures::VOLUME_B,
                fixtures::ANGLE_A,
                fixtures::ANGLE_B,
                fixtures::LEVELS,
            ]
            .concat(),
        );
        assert_eq!((s.user_a.volume, s.user_b.volume), (Some(50), Some(50)));
        assert_eq!(s.user_a.head_angle["A"], "0");
        assert_eq!(s.user_b.head_angle["E"], "0");
        assert_eq!(s.levels.len(), 32);
        assert_eq!(s.levels[0], ("L1".to_string(), 92));
        assert_eq!(s.levels[2], ("L3".to_string(), 0));
        assert_eq!(s.levels[24], ("AHL".to_string(), 53));
        assert!(s.unknown.is_empty());
    }

    #[test]
    fn tracks_speaker_sets() {
        let s = status(fixtures::VISIBILITY);
        let ids = [1, 2, 3, 4, 17, 18, 9, 10, 7, 8, 11, 25, 26, 29, 30, 24];
        assert_eq!(s.visible, ids);
        assert_eq!(s.active, ids);
        assert_eq!(s.mode.as_deref(), Some("SVS"));
        assert_eq!(s.all.as_deref(), Some("SOLO"));
        // A select in solo mode, then mute toggles.
        let s = status(&[fixtures::ACTIVE, b"A00\0A05\0\r", b"M07\0A09\0\r"].concat());
        assert_eq!(s.active, [5, 9]);
        assert_eq!(s.muted, [7]);
        assert_eq!(s.test.as_deref(), Some("OFF"));
    }
}
