// SPDX-License-Identifier: MIT
//! The playback profiles: the generated catalog of USB slot maps, and the speakers a user's
//! configuration leaves out of them.

use std::ffi::c_char;

use pipewire::spa::sys as spa;

#[rustfmt::skip]
#[path = "profiles_generated.rs"]
mod profiles_generated;

/// A playback layout: which USB slot each graph channel goes to.
#[derive(Debug)]
pub struct Profile {
    pub name: &'static str,
    pub description: &'static str,
    /// The channel positions as `audio.position` has them: `[ FL FR ... ]`.
    pub positions: &'static str,
    pub room: &'static str,
    /// The A16 speaker identities, one per graph channel, space separated.
    pub speakers: &'static str,
    pub advanced: bool,
    pub custom: bool,
    pub pulse_compatible: bool,
    pub available: bool,
    /// Per graph channel, the USB slot (0-15).
    pub usb: &'static [u8],
    /// Per graph channel, the SPA channel position.
    pub position: &'static [u32],
}

/// Every profile, in the order the card lists them.
pub fn all() -> &'static [Profile] {
    &profiles_generated::PROFILES
}

/// The index of the profile `name` names, directly or through an alias.
pub fn find(name: &str) -> Option<usize> {
    if let Some(i) = all().iter().position(|p| p.name == name) {
        return Some(i);
    }
    profiles_generated::ALIASES
        .iter()
        .find(|(alias, _)| *alias == name)
        .and_then(|(_, target)| find(target))
}

/// Is `name` one of the space-separated `words`?
pub fn word_in(words: &str, name: &str) -> bool {
    words.split_ascii_whitespace().any(|w| w == name)
}

/// The speaker identities a user's room lacks, which every profile leaves silent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Missing {
    names: Vec<String>,
    /// The identities, comma separated.
    text: String,
}

/// The most identities a list holds.
const MISSING_MAX: usize = 64;
/// The longest identity, in bytes.
const NAME_MAX: usize = 15;

impl Missing {
    pub fn count(&self) -> usize {
        self.names.len()
    }

    /// The identities, comma separated.
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn has(&self, name: &str) -> bool {
        self.names.iter().any(|n| n == name)
    }

    /// Add `name`, which a profile must know; false when no profile does or the list is full.
    pub fn add(&mut self, name: &str) -> bool {
        let known = all().iter().any(|p| word_in(p.speakers, name));
        if !known || name.is_empty() || name.len() > NAME_MAX || self.names.len() >= MISSING_MAX {
            return false;
        }
        if self.has(name) {
            return true;
        }
        self.names.push(name.to_owned());
        if !self.text.is_empty() {
            self.text.push(',');
        }
        self.text.push_str(name);
        true
    }

    /// Add the identities of `text`, separated by commas or white space.
    pub fn parse(&mut self, text: &str) -> bool {
        let separator = |c: u8| c == b',' || c.is_ascii_whitespace() || c == 0x0b;
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            while i < bytes.len() && separator(bytes[i]) {
                i += 1;
            }
            if i == bytes.len() {
                break;
            }
            let start = i;
            while i < bytes.len() && !separator(bytes[i]) {
                if i - start + 1 > NAME_MAX {
                    return false;
                }
                i += 1;
            }
            let Ok(word) = std::str::from_utf8(&bytes[start..i]) else {
                return false;
            };
            if !self.add(word) {
                return false;
            }
        }
        true
    }

    /// Add the identities of a configuration file's JSON, `{"missing_speakers": [...]}`, read
    /// as PipeWire's SPA JSON parser reads it.
    pub fn parse_json(&mut self, text: &[u8]) -> bool {
        // SAFETY: the iterators read only `text`, which outlives them, and write only into
        // the buffers whose sizes they are given
        unsafe {
            let mut root: spa::spa_json = std::mem::zeroed();
            let mut object: spa::spa_json = std::mem::zeroed();
            let mut array: spa::spa_json = std::mem::zeroed();
            let mut key = [0 as c_char; 64];
            let mut name = [0 as c_char; NAME_MAX + 1];
            let mut value: *const c_char = std::ptr::null();
            let mut seen = false;
            spa::spa_json_init(&mut root, text.as_ptr().cast(), text.len());
            if spa::spa_json_enter_object(&mut root, &mut object) <= 0 {
                return false;
            }
            let mut len;
            loop {
                len = spa::spa_json_object_next(
                    &mut object,
                    key.as_mut_ptr(),
                    key.len() as i32,
                    &mut value,
                );
                if len <= 0 {
                    break;
                }
                let key_text = std::ffi::CStr::from_ptr(key.as_ptr());
                if key_text.to_bytes() != b"missing_speakers"
                    || seen
                    || !spa::spa_json_is_array(value, len)
                {
                    return false;
                }
                seen = true;
                spa::spa_json_enter(&mut object, &mut array);
                loop {
                    len = spa::spa_json_next(&mut array, &mut value);
                    if len <= 0 {
                        break;
                    }
                    if !spa::spa_json_is_string(value, len)
                        || spa::spa_json_parse_stringn(
                            value,
                            len,
                            name.as_mut_ptr(),
                            name.len() as i32,
                        ) <= 0
                    {
                        return false;
                    }
                    let parsed = std::ffi::CStr::from_ptr(name.as_ptr());
                    match parsed.to_str() {
                        Ok(parsed) if self.add(parsed) => {}
                        _ => return false,
                    }
                }
                if len < 0 {
                    return false;
                }
            }
            len == 0
        }
    }

    /// Add the identities of the configuration file at `path`. A missing file is no error
    /// when `optional`.
    pub fn load(&mut self, path: &std::path::Path, optional: bool) -> bool {
        use std::io::Read as _;
        let mut file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(error) => return optional && error.kind() == std::io::ErrorKind::NotFound,
        };
        // A configuration holds at most 4096 bytes
        let mut buffer = Vec::with_capacity(4097);
        match file.by_ref().take(4097).read_to_end(&mut buffer) {
            Ok(n) if n < 4097 => self.parse_json(&buffer),
            _ => false,
        }
    }
}

/// A profile without the speakers a user lacks: the remaining channels keep their USB slots.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Effective {
    pub name: &'static str,
    pub description: String,
    pub positions: String,
    pub room: &'static str,
    pub speakers: String,
    pub advanced: bool,
    pub custom: bool,
    pub pulse_compatible: bool,
    pub available: bool,
    pub usb: Vec<u8>,
    pub position: Vec<u32>,
}

impl Effective {
    /// `base` without the speakers of `missing`; None when its speaker and position lists
    /// disagree.
    pub fn new(base: &Profile, missing: &Missing) -> Option<Effective> {
        let mut labels = base.speakers.split(' ').filter(|w| !w.is_empty());
        let mut positions = base
            .positions
            .split(['[', ']', ' '])
            .filter(|w| !w.is_empty());
        let mut out = Effective {
            name: base.name,
            description: base.description.to_owned(),
            positions: "[".to_owned(),
            room: base.room,
            speakers: String::new(),
            advanced: base.advanced,
            custom: base.custom,
            pulse_compatible: base.pulse_compatible,
            available: false,
            usb: Vec::new(),
            position: Vec::new(),
        };
        for i in 0..base.usb.len() {
            let (label, position) = (labels.next()?, positions.next()?);
            if !missing.has(label) {
                out.usb.push(base.usb[i]);
                out.position.push(base.position[i]);
                if !out.speakers.is_empty() {
                    out.speakers.push(' ');
                }
                out.speakers.push_str(label);
                out.positions.push(' ');
                out.positions.push_str(position);
            }
        }
        if labels.next().is_some() || positions.next().is_some() {
            return None;
        }
        out.positions.push_str(" ]");
        out.available = base.available && !out.usb.is_empty();
        if out.usb.len() != base.usb.len() {
            out.description = format!(
                "{} [{}/{} speakers]",
                base.description,
                out.usb.len(),
                base.usb.len()
            );
        }
        Some(out)
    }

    pub fn channels(&self) -> usize {
        self.usb.len()
    }

    /// One frame of this profile's channels as a 16-slot USB frame, unused slots silent.
    pub fn expand(&self, out: &mut [f32], input: &[f32]) {
        out[..16].fill(0.0);
        for (c, &slot) in self.usb.iter().enumerate() {
            out[usize::from(slot)] = input[c];
        }
    }
}

/// Every profile without the speakers of `missing`, in catalog order.
pub fn effective(missing: &Missing) -> Option<Vec<Effective>> {
    all().iter().map(|p| Effective::new(p, missing)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_and_missing_speakers() {
        assert_eq!((find("7.1.4"), find("9.1.6")), (Some(0), Some(1)));
        assert_eq!(find("invalid-layout"), None);
        assert!(all().len() > 40);
        assert_eq!((find("7.1.4t"), find("9.1.6t")), (Some(0), Some(1)));
        assert!(find("7.1.4h").is_some() && find("9.1").is_some());
        assert!(all()[find("9.1").expect("9.1")].advanced);
        assert!(all()[0].usb.len() == 12 && !all()[0].advanced);
        assert!(all()[1].advanced);

        let mut missing = Missing::default();
        assert!(missing.parse("Chr"));
        assert!(!missing.parse("NotASpeaker"));
        let mut parsed = Missing::default();
        assert!(parsed.parse_json(br#"{"missing_speakers":["Chr"]}"#) && parsed.count() == 1);
        assert!(!parsed.parse_json(br#"{"missing_speakers":12}"#));

        for bank in 1..=4 {
            let base = &all()[find(&format!("custom-{bank}")).expect("bank")];
            let effective = Effective::new(base, &missing).expect("effective");
            assert_eq!(effective.channels(), if bank == 1 { 15 } else { 16 });
            for c in 0..effective.channels() {
                let original = base
                    .usb
                    .iter()
                    .position(|&u| u == effective.usb[c])
                    .expect("slot");
                assert_eq!(base.position[original], effective.position[c]);
            }
            let (input, mut output) = ([1.0f32; 16], [0.0f32; 16]);
            effective.expand(&mut output, &input);
            if bank == 1 {
                assert!(output[13] == 0.0 && output[14] == 1.0 && output[15] == 1.0);
            }
            assert!(base.usb.len() == 16 && word_in(base.speakers, "Chr") == (bank == 1));
        }

        let mut every = Missing::default();
        assert!(every.parse("L,R"));
        let effective = Effective::new(&all()[find("2.0").expect("2.0")], &every).expect("2.0");
        assert!(!effective.available && effective.channels() == 0);
    }

    #[test]
    fn every_profile() {
        let none = Missing::default();
        let allowed = [
            spa::SPA_AUDIO_CHANNEL_FL,
            spa::SPA_AUDIO_CHANNEL_FR,
            spa::SPA_AUDIO_CHANNEL_FC,
            spa::SPA_AUDIO_CHANNEL_LFE,
            spa::SPA_AUDIO_CHANNEL_SL,
            spa::SPA_AUDIO_CHANNEL_SR,
            spa::SPA_AUDIO_CHANNEL_RL,
            spa::SPA_AUDIO_CHANNEL_RR,
            spa::SPA_AUDIO_CHANNEL_RC,
            spa::SPA_AUDIO_CHANNEL_FLC,
            spa::SPA_AUDIO_CHANNEL_FRC,
            spa::SPA_AUDIO_CHANNEL_TC,
            spa::SPA_AUDIO_CHANNEL_TFL,
            spa::SPA_AUDIO_CHANNEL_TFC,
            spa::SPA_AUDIO_CHANNEL_TFR,
            spa::SPA_AUDIO_CHANNEL_TRL,
            spa::SPA_AUDIO_CHANNEL_TRC,
            spa::SPA_AUDIO_CHANNEL_TRR,
        ];
        let mut custom = 0;
        for (mode, p) in all().iter().enumerate() {
            assert!(p.usb.len() <= 16 && p.usb.len() == p.position.len());
            assert_eq!(p.available, !p.usb.is_empty());
            assert!(p.available || p.custom);
            assert!(!p.custom || p.pulse_compatible);
            if p.custom {
                custom += 1;
                assert!(p.name.starts_with("custom-") && p.advanced);
            }
            if p.pulse_compatible {
                assert!(p.position.iter().all(|c| allowed.contains(c)));
            }
            assert_eq!(find(p.name), Some(mode));
            for i in 0..p.usb.len() {
                assert!(p.usb[i] < 16);
                for j in i + 1..p.usb.len() {
                    assert!(p.usb[i] != p.usb[j] && p.position[i] != p.position[j]);
                }
            }
            assert!(all()[mode + 1..].iter().all(|q| q.name != p.name));
            let effective = Effective::new(p, &none).expect("effective");
            assert_eq!(effective.positions, p.positions);
            assert_eq!(effective.speakers, p.speakers);
            for channel in 0..p.usb.len() {
                let mut input = [0.0f32; 16];
                input[channel] = 0.5;
                let mut out = [0.0f32; 16];
                effective.expand(&mut out, &input);
                for (usb, &v) in out.iter().enumerate() {
                    let expected = if usb == usize::from(p.usb[channel]) {
                        0.5
                    } else {
                        0.0
                    };
                    assert_eq!(v, expected);
                }
            }
        }
        assert_eq!(custom, 4);
        for bank in 1..=4 {
            let p = &all()[find(&format!("custom-{bank}")).expect("bank")];
            assert!(p.available && p.custom && p.pulse_compatible && p.usb.len() == 16);
            let has = |c| p.position.contains(&c);
            assert_eq!(has(spa::SPA_AUDIO_CHANNEL_TC), bank != 4);
            assert_eq!(
                has(spa::SPA_AUDIO_CHANNEL_FLC) && has(spa::SPA_AUDIO_CHANNEL_FRC),
                bank != 1
            );
        }
    }
}
