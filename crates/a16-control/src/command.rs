// SPDX-License-Identifier: MIT
//! The commands of the A16's TCP command server: Table 1 (the IR remote keys) and Table 2
//! (the other functions) of Smyth's command summary.

use std::fmt;

use crate::parser::Item;
use crate::speaker::SpeakerId;

/// One of the two listeners of the A16. Each has a preset and a volume.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum User {
    A,
    B,
}

impl User {
    pub fn parse(text: &str) -> Option<User> {
        match text {
            "a" | "A" => Some(User::A),
            "b" | "B" => Some(User::B),
            _ => None,
        }
    }

    pub(crate) fn pick<T>(self, a: T, b: T) -> T {
        match self {
            User::A => a,
            User::B => b,
        }
    }
}

impl fmt::Display for User {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.pick("A", "B"))
    }
}

macro_rules! keys {
    ($($variant:ident = $code:literal, $name:literal;)*) => {
        /// A key of the IR remote, as Table 1 lists them.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Key { $($variant),* }

        impl Key {
            /// Every key, in the order of Table 1.
            pub const ALL: &'static [Key] = &[$(Key::$variant),*];

            pub fn code(self) -> u8 {
                match self { $(Key::$variant => $code),* }
            }

            /// The name `a16-ctl send key` takes.
            pub fn name(self) -> &'static str {
                match self { $(Key::$variant => $name),* }
            }
        }
    };
}

keys! {
    Power = 0x11, "power";
    Menu = 0x43, "menu";
    Back = 0x59, "back";
    Enter = 0x04, "enter";
    Cancel = 0x50, "cancel";
    AdjUp = 0x56, "adj+";
    AdjDown = 0x5a, "adj-";
    Up = 0x47, "up";
    Down = 0x4b, "down";
    Left = 0x57, "left";
    Right = 0x53, "right";
    Digit1 = 0x5d, "1";
    Digit2 = 0x5f, "2";
    Digit3 = 0x49, "3";
    Digit4 = 0x16, "4";
    Digit5 = 0x17, "5";
    Digit6 = 0x44, "6";
    Digit7 = 0x55, "7";
    Digit8 = 0x0f, "8";
    Digit9 = 0x0e, "9";
    Digit0 = 0x5b, "0";
    Star = 0x12, "*";
    Hash = 0x36, "#";
    Cal = 0x03, "cal";
    Spk = 0x0b, "spk";
    Hp = 0x0a, "hp";
    C = 0x1c, "c";
    L = 0x1d, "l";
    R = 0x46, "r";
    Lw = 0x0d, "lw";
    Rw = 0x0c, "rw";
    Ls = 0x15, "ls";
    Rs = 0x45, "rs";
    Lb = 0x00, "lb";
    Rb = 0x1b, "rb";
    Lfe = 0x4c, "lfe";
    Oh = 0x14, "oh";
    All = 0x1a, "all";
    Headphone = 0x4d, "headphone";
    Speaker = 0x5e, "speaker";
    VolumeUpA = 0x30, "a-vol+";
    VolumeDownA = 0x31, "a-vol-";
    VolumeUpB = 0x33, "b-vol+";
    VolumeDownB = 0x34, "b-vol-";
    MuteA = 0x32, "mute-a";
    MuteB = 0x35, "mute-b";
    Test = 0x08, "test";
    Ht = 0x42, "ht";
    Pa = 0x58, "pa";
    Pb = 0x54, "pb";
}

impl Key {
    pub fn from_name(name: &str) -> Option<Key> {
        Key::ALL
            .iter()
            .copied()
            .find(|k| k.name().eq_ignore_ascii_case(name))
    }

    pub fn from_code(code: u8) -> Option<Key> {
        Key::ALL.iter().copied().find(|k| k.code() == code)
    }

    /// The key of the item that answers this key press, for the keys that do not answer
    /// `KEY=<code>`.
    fn reply_key(self) -> Option<&'static str> {
        Some(match self {
            Key::Back => "PAGE",
            Key::All => "ALL",
            Key::Headphone | Key::Speaker => "MODE",
            Key::VolumeUpA | Key::VolumeDownA => "VA",
            Key::VolumeUpB | Key::VolumeDownB => "VB",
            Key::MuteA => "MUTEA",
            Key::MuteB => "MUTEB",
            Key::Test => "TEST",
            Key::Ht => "HT",
            _ => return None,
        })
    }
}

/// An audio source, codes `20` to `29`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Earc,
    Hdmi1,
    Hdmi2,
    Hdmi3,
    Hdmi4,
    Usb,
    Line,
    Stereo,
    Coaxial,
    Optical,
}

impl Input {
    /// Every input, in code order from `20`.
    pub const ALL: [Input; 10] = [
        Input::Earc,
        Input::Hdmi1,
        Input::Hdmi2,
        Input::Hdmi3,
        Input::Hdmi4,
        Input::Usb,
        Input::Line,
        Input::Stereo,
        Input::Coaxial,
        Input::Optical,
    ];

    /// The value of the `IN` item the A16 answers with.
    pub fn name(self) -> &'static str {
        match self {
            Input::Earc => "eARC",
            Input::Hdmi1 => "HDMI-1",
            Input::Hdmi2 => "HDMI-2",
            Input::Hdmi3 => "HDMI-3",
            Input::Hdmi4 => "HDMI-4",
            Input::Usb => "USB",
            Input::Line => "LINE",
            Input::Stereo => "STEREO",
            Input::Coaxial => "COAXIAL",
            Input::Optical => "OPTICAL",
        }
    }

    /// The input of this name, ignoring case and the hyphen of `hdmi-1`.
    pub fn parse(text: &str) -> Option<Input> {
        let squash = |s: &str| s.replace('-', "").to_ascii_lowercase();
        let text = squash(text);
        Input::ALL.into_iter().find(|i| squash(i.name()) == text)
    }

    fn code(self) -> u8 {
        0x20 + self as u8
    }
}

/// The power functions, codes `2c` to `2e`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Power {
    On,
    Standby,
    Status,
}

/// A direction of a volume step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Up,
    Down,
}

/// A preset number, 1 to 16.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preset(u8);

impl Preset {
    pub fn new(number: u8) -> Option<Preset> {
        (1..=16).contains(&number).then_some(Preset(number))
    }

    pub fn get(self) -> u8 {
        self.0
    }
}

/// A command the A16 takes over TCP.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// An IR remote key press (Table 1).
    Key(Key),
    Input(Input),
    Power(Power),
    /// Load a preset of a user: `70`-`7f` for A, `90`-`9f` for B.
    Preset(User, Preset),
    /// One volume step, the `A vol±` and `B vol±` keys.
    Volume(User, Step),
    /// Toggle the mute of a user, the `MUTE A` and `MUTE B` keys.
    Mute(User),
    /// Request the info of a user: `80` or `a0`.
    Info(User),
    /// Request the volume of a user: `83` or `a3`.
    VolumeLevel(User),
    /// Request the head-tracker angle of a user: `85` or `a5`.
    HeadAngle(User),
    /// Request the speaker of each channel of both users: `37`.
    Assignments,
    /// Reset all level meters: `38`.
    ResetLevels,
    /// Request the firmware versions and serial number: `64`.
    Firmware,
    /// Request all levels: `6a`.
    Levels,
    /// Request the speaker keys shown: `ae`.
    Visibility,
    /// Request the active speakers: `af`.
    ActiveSpeakers,
    /// Select a speaker, which solos or mutes it by the ALL mode: `B1` to `E2`.
    SelectSpeaker(SpeakerId),
    /// A code with no function in the command summary.
    Raw(u8),
}

impl Command {
    pub fn code(self) -> u8 {
        match self {
            Command::Key(key) => key.code(),
            Command::Input(input) => input.code(),
            Command::Power(Power::On) => 0x2c,
            Command::Power(Power::Standby) => 0x2d,
            Command::Power(Power::Status) => 0x2e,
            Command::Preset(user, preset) => user.pick(0x70, 0x90) + preset.0 - 1,
            Command::Volume(user, Step::Up) => user.pick(Key::VolumeUpA, Key::VolumeUpB).code(),
            Command::Volume(user, Step::Down) => {
                user.pick(Key::VolumeDownA, Key::VolumeDownB).code()
            }
            Command::Mute(user) => user.pick(Key::MuteA, Key::MuteB).code(),
            Command::Info(user) => user.pick(0x80, 0xa0),
            Command::VolumeLevel(user) => user.pick(0x83, 0xa3),
            Command::HeadAngle(user) => user.pick(0x85, 0xa5),
            Command::Assignments => 0x37,
            Command::ResetLevels => 0x38,
            Command::Firmware => 0x64,
            Command::Levels => 0x6a,
            Command::Visibility => 0xae,
            Command::ActiveSpeakers => 0xaf,
            Command::SelectSpeaker(id) => 0xb0 + id.get(),
            Command::Raw(code) => code,
        }
    }

    /// The bytes on the wire: two lowercase hex digits and CR LF.
    pub fn encode(self) -> [u8; 4] {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let code = self.code();
        [
            HEX[usize::from(code >> 4)],
            HEX[usize::from(code & 15)],
            b'\r',
            b'\n',
        ]
    }

    /// The command of a code. The volume and mute keys decode as [`Command::Key`].
    pub fn from_code(code: u8) -> Command {
        let user = if code >= 0xa0 { User::B } else { User::A };
        match code {
            0x20..=0x29 => Command::Input(Input::ALL[usize::from(code - 0x20)]),
            0x2c => Command::Power(Power::On),
            0x2d => Command::Power(Power::Standby),
            0x2e => Command::Power(Power::Status),
            0x37 => Command::Assignments,
            0x38 => Command::ResetLevels,
            0x64 => Command::Firmware,
            0x6a => Command::Levels,
            0x70..=0x7f => Command::Preset(User::A, Preset(code - 0x6f)),
            0x90..=0x9f => Command::Preset(User::B, Preset(code - 0x8f)),
            0x80 | 0xa0 => Command::Info(user),
            0x83 | 0xa3 => Command::VolumeLevel(user),
            0x85 | 0xa5 => Command::HeadAngle(user),
            0xae => Command::Visibility,
            0xaf => Command::ActiveSpeakers,
            0xb1..=0xe2 => Command::SelectSpeaker(SpeakerId::new(code - 0xb0).unwrap()),
            _ => Key::from_code(code).map_or(Command::Raw(code), Command::Key),
        }
    }

    /// Whether this item belongs to the reply of the A16 to the command. Items of other
    /// functions may arrive meanwhile, from the IR remote or the front panel. `Raw` matches
    /// nothing.
    pub fn answered_by(self, item: &Item) -> bool {
        let key = |want: &str| item.key() == Some(want);
        match self {
            Command::Key(press) => match press.reply_key() {
                Some(reply) => key(reply),
                None => {
                    key("KEY") && u8::from_str_radix(item.value(), 16).ok() == Some(press.code())
                }
            },
            Command::Volume(user, _) | Command::VolumeLevel(user) => key(user.pick("VA", "VB")),
            Command::Mute(user) => key(user.pick("MUTEA", "MUTEB")),
            Command::Input(_) => key("IN"),
            Command::Power(_) => key("PWR"),
            Command::Preset(user, _) => key(user.pick("PA", "PB")),
            Command::Info(user) => key(user.pick("AUR", "BUR")),
            Command::HeadAngle(user) => item
                .key()
                .is_some_and(|k| k.starts_with(user.pick("AHT", "BHT"))),
            Command::Assignments => key("ACH1") || key("BCH1"),
            Command::ResetLevels | Command::Levels => key("L1"),
            Command::Firmware => key("A16FW"),
            Command::Visibility => item.token() == Some("V00"),
            Command::ActiveSpeakers => item.token() == Some("A00"),
            Command::SelectSpeaker(id) => {
                let n = format!("{:02}", id.get());
                item.token().is_some_and(|t| {
                    (t.starts_with('A') || t.starts_with('M')) && t.get(1..) == Some(n.as_str())
                })
            }
            Command::Raw(_) => false,
        }
    }

    /// The command that the words of `a16-ctl send` name.
    pub fn parse(words: &[&str]) -> Result<Command, String> {
        let user =
            |word: &str| User::parse(word).ok_or_else(|| format!("expected a or b, not {word}"));
        let command = match *words {
            ["key", name] => {
                Command::Key(Key::from_name(name).ok_or(format!("unknown key: {name}"))?)
            }
            ["input", name] => {
                Command::Input(Input::parse(name).ok_or(format!("unknown input: {name}"))?)
            }
            ["power", "on"] => Command::Power(Power::On),
            ["power", "standby"] => Command::Power(Power::Standby),
            ["power", "status"] => Command::Power(Power::Status),
            ["preset", u, n] => Command::Preset(
                user(u)?,
                n.parse()
                    .ok()
                    .and_then(Preset::new)
                    .ok_or(format!("presets are 1 to 16, not {n}"))?,
            ),
            ["volume", u] => Command::VolumeLevel(user(u)?),
            ["volume", u, "up"] => Command::Volume(user(u)?, Step::Up),
            ["volume", u, "down"] => Command::Volume(user(u)?, Step::Down),
            ["mute", u] => Command::Mute(user(u)?),
            ["info", u] => Command::Info(user(u)?),
            ["angle", u] => Command::HeadAngle(user(u)?),
            ["assignments"] => Command::Assignments,
            ["reset-levels"] => Command::ResetLevels,
            ["firmware"] => Command::Firmware,
            ["levels"] => Command::Levels,
            ["visibility"] => Command::Visibility,
            ["active"] => Command::ActiveSpeakers,
            ["speaker", which] => Command::SelectSpeaker(
                SpeakerId::parse(which)
                    .ok_or(format!("no selectable speaker {which} (IDs 1 to 50)"))?,
            ),
            _ => return Err(format!("unknown command: {}", words.join(" "))),
        };
        Ok(command)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire(command: Command) -> String {
        String::from_utf8(command.encode().to_vec()).unwrap()
    }

    #[test]
    fn encodes_codes_as_two_hex_digits_and_crlf() {
        assert_eq!(wire(Command::Assignments), "37\r\n");
        assert_eq!(wire(Command::Key(Key::Lb)), "00\r\n");
        assert_eq!(wire(Command::Key(Key::AdjDown)), "5a\r\n");
        assert_eq!(wire(Command::Input(Input::Earc)), "20\r\n");
        assert_eq!(wire(Command::Input(Input::Usb)), "25\r\n");
        assert_eq!(wire(Command::Input(Input::Optical)), "29\r\n");
        assert_eq!(wire(Command::Power(Power::On)), "2c\r\n");
        assert_eq!(wire(Command::Power(Power::Standby)), "2d\r\n");
        assert_eq!(wire(Command::Power(Power::Status)), "2e\r\n");
        assert_eq!(wire(Command::ResetLevels), "38\r\n");
        assert_eq!(wire(Command::Firmware), "64\r\n");
        assert_eq!(wire(Command::Levels), "6a\r\n");
        assert_eq!(wire(Command::Visibility), "ae\r\n");
        assert_eq!(wire(Command::ActiveSpeakers), "af\r\n");
    }

    #[test]
    fn encodes_per_user_commands() {
        let p = |n| Preset::new(n).unwrap();
        assert_eq!(wire(Command::Preset(User::A, p(1))), "70\r\n");
        assert_eq!(wire(Command::Preset(User::A, p(16))), "7f\r\n");
        assert_eq!(wire(Command::Preset(User::B, p(1))), "90\r\n");
        assert_eq!(wire(Command::Preset(User::B, p(16))), "9f\r\n");
        assert_eq!(wire(Command::Volume(User::A, Step::Up)), "30\r\n");
        assert_eq!(wire(Command::Volume(User::A, Step::Down)), "31\r\n");
        assert_eq!(wire(Command::Volume(User::B, Step::Up)), "33\r\n");
        assert_eq!(wire(Command::Volume(User::B, Step::Down)), "34\r\n");
        assert_eq!(wire(Command::Mute(User::A)), "32\r\n");
        assert_eq!(wire(Command::Mute(User::B)), "35\r\n");
        assert_eq!(wire(Command::Info(User::A)), "80\r\n");
        assert_eq!(wire(Command::Info(User::B)), "a0\r\n");
        assert_eq!(wire(Command::VolumeLevel(User::A)), "83\r\n");
        assert_eq!(wire(Command::VolumeLevel(User::B)), "a3\r\n");
        assert_eq!(wire(Command::HeadAngle(User::A)), "85\r\n");
        assert_eq!(wire(Command::HeadAngle(User::B)), "a5\r\n");
    }

    #[test]
    fn encodes_speaker_selection() {
        let select = |n| Command::SelectSpeaker(SpeakerId::new(n).unwrap());
        assert_eq!(wire(select(1)), "b1\r\n");
        assert_eq!(wire(select(50)), "e2\r\n");
    }

    #[test]
    fn key_table_has_fifty_distinct_keys() {
        assert_eq!(Key::ALL.len(), 50);
        for (i, a) in Key::ALL.iter().enumerate() {
            for b in &Key::ALL[i + 1..] {
                assert_ne!(a.code(), b.code(), "{a:?} {b:?}");
                assert_ne!(a.name(), b.name());
            }
        }
    }

    #[test]
    fn every_code_decodes_to_a_command_with_that_code() {
        for code in 0..=255u8 {
            assert_eq!(Command::from_code(code).code(), code, "{code:02x}");
        }
        assert_eq!(Command::from_code(0xa0), Command::Info(User::B));
        assert_eq!(Command::from_code(0x85), Command::HeadAngle(User::A));
        assert_eq!(Command::from_code(0x01), Command::Raw(0x01));
    }

    #[test]
    fn parses_cli_words() {
        let parse = |s: &str| Command::parse(&s.split(' ').collect::<Vec<_>>());
        assert_eq!(parse("input usb"), Ok(Command::Input(Input::Usb)));
        assert_eq!(parse("input hdmi-2"), Ok(Command::Input(Input::Hdmi2)));
        assert_eq!(parse("input eARC"), Ok(Command::Input(Input::Earc)));
        assert_eq!(parse("preset a 3").unwrap().code(), 0x72);
        assert_eq!(parse("volume a up").unwrap().code(), 0x30);
        assert_eq!(parse("volume b").unwrap().code(), 0xa3);
        assert_eq!(parse("mute b").unwrap().code(), 0x35);
        assert_eq!(parse("power standby").unwrap().code(), 0x2d);
        assert_eq!(parse("key menu").unwrap().code(), 0x43);
        assert_eq!(parse("key adj-").unwrap().code(), 0x5a);
        assert_eq!(parse("key #").unwrap().code(), 0x36);
        assert_eq!(parse("speaker Ls").unwrap().code(), 0xb5);
        assert_eq!(parse("speaker 50").unwrap().code(), 0xe2);
        assert_eq!(parse("angle b").unwrap().code(), 0xa5);
        assert!(parse("preset a 17").is_err());
        assert!(parse("preset c 1").is_err());
        assert!(parse("speaker 51").is_err());
        assert!(parse("key nope").is_err());
        assert!(parse("frobnicate").is_err());
    }

    #[test]
    fn matches_replies() {
        let item = Item::parse;
        assert!(Command::Key(Key::Menu).answered_by(&item("KEY=43")));
        assert!(Command::Key(Key::AdjDown).answered_by(&item("KEY=5A")));
        assert!(!Command::Key(Key::Menu).answered_by(&item("KEY=04")));
        assert!(Command::Key(Key::Back).answered_by(&item("PAGE=0")));
        assert!(Command::Volume(User::B, Step::Up).answered_by(&item("VB=51")));
        assert!(Command::HeadAngle(User::A).answered_by(&item("AHTA=0")));
        assert!(Command::Power(Power::Status).answered_by(&item("PWR=ON")));
        assert!(!Command::Power(Power::Status).answered_by(&item("PWRX=ON")));
        let select = Command::SelectSpeaker(SpeakerId::new(5).unwrap());
        assert!(select.answered_by(&item("A05")));
        assert!(select.answered_by(&item("M05")));
        assert!(!select.answered_by(&item("A00")));
        assert!(Command::ActiveSpeakers.answered_by(&item("A00")));
        assert!(!Command::Raw(1).answered_by(&item("A00")));
    }
}
