// SPDX-License-Identifier: MIT
//! The A16's loudspeaker IDs, from Tables 3 and 4 of Smyth's "Realiser A16 IP (TCP) Command
//! Summary".

use std::fmt;

/// The position class of a loudspeaker, as the legend of Table 4 names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Floor,
    Height,
    Overhead,
    Under,
    Lfe,
}

impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Category::Floor => "floor",
            Category::Height => "height",
            Category::Overhead => "overhead",
            Category::Under => "under",
            Category::Lfe => "LFE",
        })
    }
}

/// One loudspeaker of the A16's virtual room.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Speaker {
    pub id: u8,
    pub designator: &'static str,
    pub category: Category,
}

const fn s(id: u8, designator: &'static str, category: Category) -> Speaker {
    Speaker {
        id,
        designator,
        category,
    }
}

use Category::{Floor, Height, Lfe, Overhead, Under};

/// Every loudspeaker, in ID order: `SPEAKERS[n - 1].id == n`.
pub const SPEAKERS: [Speaker; 52] = [
    s(1, "L", Floor),
    s(2, "R", Floor),
    s(3, "C", Floor),
    s(4, "SW", Lfe),
    s(5, "Ls", Floor),
    s(6, "Rs", Floor),
    s(7, "Lb", Floor),
    s(8, "Rb", Floor),
    s(9, "Lss", Floor),
    s(10, "Rss", Floor),
    s(11, "Cr", Floor),
    s(12, "SW2", Lfe),
    s(13, "Lw", Floor),
    s(14, "Rw", Floor),
    s(15, "Lbs", Floor),
    s(16, "Rbs", Floor),
    s(17, "Lc", Floor),
    s(18, "Rc", Floor),
    s(19, "Lu", Under),
    s(20, "Ru", Under),
    s(21, "Cu", Under),
    s(22, "Ch", Height),
    s(23, "Chr", Height),
    s(24, "T", Overhead),
    s(25, "Lh", Height),
    s(26, "Rh", Height),
    s(27, "Lhs", Height),
    s(28, "Rhs", Height),
    s(29, "Lhr", Height),
    s(30, "Rhr", Height),
    s(31, "Ltf", Overhead),
    s(32, "Rtf", Overhead),
    s(33, "Ltm", Overhead),
    s(34, "Rtm", Overhead),
    s(35, "Ltr", Overhead),
    s(36, "Rtr", Overhead),
    s(37, "Lsc", Floor),
    s(38, "Rsc", Floor),
    s(39, "Ls1", Floor),
    s(40, "Rs1", Floor),
    s(41, "Lrs1", Floor),
    s(42, "Rrs1", Floor),
    s(43, "Lrs2", Floor),
    s(44, "Rrs2", Floor),
    s(45, "Lhw", Height),
    s(46, "Rhw", Height),
    s(47, "Lhs1", Height),
    s(48, "Rhs1", Height),
    s(49, "Lbu", Under),
    s(50, "Rbu", Under),
    s(51, "SW", Lfe),
    s(52, "SW", Lfe),
];

/// The loudspeaker with this ID.
pub fn by_id(id: u8) -> Option<&'static Speaker> {
    SPEAKERS.get(usize::from(id).checked_sub(1)?)
}

/// The loudspeaker with this designator, ignoring case. `SW` names ID 4, the first of the three.
pub fn by_designator(designator: &str) -> Option<&'static Speaker> {
    SPEAKERS
        .iter()
        .find(|s| s.designator.eq_ignore_ascii_case(designator))
}

/// A loudspeaker the select commands reach: IDs 1 to 50, codes `B1` to `E2`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpeakerId(u8);

impl SpeakerId {
    pub fn new(id: u8) -> Option<SpeakerId> {
        (1..=50).contains(&id).then_some(SpeakerId(id))
    }

    /// The ID, or the designator, of a selectable loudspeaker.
    pub fn parse(text: &str) -> Option<SpeakerId> {
        match text.parse::<u8>() {
            Ok(id) => SpeakerId::new(id),
            Err(_) => SpeakerId::new(by_designator(text)?.id),
        }
    }

    pub fn get(self) -> u8 {
        self.0
    }

    pub fn speaker(self) -> &'static Speaker {
        &SPEAKERS[usize::from(self.0) - 1]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_in_id_order() {
        for (index, speaker) in SPEAKERS.iter().enumerate() {
            assert_eq!(usize::from(speaker.id), index + 1);
        }
    }

    #[test]
    fn designators_resolve() {
        assert_eq!(by_designator("ls").unwrap().id, 5);
        assert_eq!(by_designator("SW").unwrap().id, 4);
        assert_eq!(by_designator("SW2").unwrap().id, 12);
        assert_eq!(by_designator("Rbu").unwrap().category, Category::Under);
        assert!(by_designator("Xx").is_none());
        assert_eq!(by_id(24).unwrap().designator, "T");
        assert!(by_id(0).is_none() && by_id(53).is_none());
    }

    #[test]
    fn selectable_ids() {
        assert_eq!(SpeakerId::parse("Ls").unwrap().get(), 5);
        assert_eq!(SpeakerId::parse("50").unwrap().speaker().designator, "Rbu");
        assert!(SpeakerId::parse("51").is_none());
        assert!(SpeakerId::parse("0").is_none());
        assert!(SpeakerId::parse("nope").is_none());
    }
}
