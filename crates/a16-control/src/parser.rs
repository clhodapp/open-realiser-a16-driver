// SPDX-License-Identifier: MIT
//! The framing of the A16's replies: ASCII items, each ended by a NUL. On firmware 2.17 every
//! packet also ends with a CR after its last NUL, and some packets carry runs of empty items.

use std::fmt;

/// One item of a reply: `KEY=value`, or a bare token such as `A01`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Field { key: String, value: String },
    Token(String),
}

impl Item {
    /// The item of this text, split at its first `=`.
    pub fn parse(text: &str) -> Item {
        match text.split_once('=') {
            Some((key, value)) => Item::Field {
                key: key.to_string(),
                value: value.to_string(),
            },
            None => Item::Token(text.to_string()),
        }
    }

    pub fn key(&self) -> Option<&str> {
        match self {
            Item::Field { key, .. } => Some(key),
            Item::Token(_) => None,
        }
    }

    /// The value of a field, or the text of a token.
    pub fn value(&self) -> &str {
        match self {
            Item::Field { value, .. } => value,
            Item::Token(token) => token,
        }
    }

    pub fn token(&self) -> Option<&str> {
        match self {
            Item::Token(token) => Some(token),
            Item::Field { .. } => None,
        }
    }
}

impl fmt::Display for Item {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Item::Field { key, value } => write!(f, "{key}={value}"),
            Item::Token(token) => f.write_str(token),
        }
    }
}

/// What the parser finds in the byte stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Item(Item),
    /// A CR right after a NUL: the end of a packet.
    PacketEnd,
}

/// Splits the byte stream into items. Bytes may arrive in any pieces.
#[derive(Debug)]
pub struct Parser {
    partial: Vec<u8>,
    after_nul: bool,
}

impl Default for Parser {
    fn default() -> Self {
        Parser::new()
    }
}

impl Parser {
    pub fn new() -> Parser {
        Parser {
            partial: Vec::new(),
            after_nul: false,
        }
    }

    /// The events these bytes complete. Empty items are dropped, and so are CR and LF at the
    /// start of an item.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Event> {
        let mut events = Vec::new();
        for &byte in bytes {
            match byte {
                0 => {
                    if !self.partial.is_empty() {
                        // Latin-1, so that no byte is lost.
                        let text: String = self.partial.iter().map(|&b| char::from(b)).collect();
                        events.push(Event::Item(Item::parse(&text)));
                        self.partial.clear();
                    }
                    self.after_nul = true;
                    continue;
                }
                b'\r' | b'\n' if self.partial.is_empty() => {
                    if byte == b'\r' && self.after_nul {
                        events.push(Event::PacketEnd);
                    }
                }
                _ => self.partial.push(byte),
            }
            self.after_nul = false;
        }
        events
    }

    /// Whether bytes of an unfinished item are waiting for their NUL.
    pub fn has_partial(&self) -> bool {
        !self.partial.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(events: &[Event]) -> Vec<String> {
        events
            .iter()
            .map(|e| match e {
                Event::Item(item) => item.to_string(),
                Event::PacketEnd => "<end>".to_string(),
            })
            .collect()
    }

    #[test]
    fn splits_items_and_packets() {
        let mut parser = Parser::new();
        let events = parser.feed(b"PWR=ON\0\rALL=SOLO\0A01\0\r");
        assert_eq!(
            items(&events),
            ["PWR=ON", "<end>", "ALL=SOLO", "A01", "<end>"]
        );
        assert_eq!(events[0], Event::Item(Item::parse("PWR=ON")));
        assert_eq!(
            Item::parse("A01"),
            Item::Token("A01".to_string()),
            "a bare token"
        );
    }

    #[test]
    fn drops_padding() {
        let mut parser = Parser::new();
        let events = parser.feed(b"\0VB=50\0\0\0\0HSRFW=v74\0\r");
        assert_eq!(items(&events), ["VB=50", "HSRFW=v74", "<end>"]);
    }

    #[test]
    fn joins_items_split_across_reads() {
        let stream = b"A16FW=2.17 Jan 26 2024\0APMFW=APM\0\0\0\r\0VB=50\0\r";
        let whole = items(&Parser::new().feed(stream));
        for size in 1..stream.len() {
            let mut parser = Parser::new();
            let mut got = Vec::new();
            for chunk in stream.chunks(size) {
                got.extend(items(&parser.feed(chunk)));
            }
            assert_eq!(got, whole, "chunks of {size}");
            assert!(!parser.has_partial());
        }
        assert_eq!(
            whole,
            [
                "A16FW=2.17 Jan 26 2024",
                "APMFW=APM",
                "<end>",
                "VB=50",
                "<end>"
            ]
        );
    }

    #[test]
    fn keeps_values_verbatim() {
        let mut parser = Parser::new();
        let events = parser.feed(b"ACH17= \0L3=0 \0AQFILE=Chris  _Focal Clear \0A16SN=x=y\0");
        assert_eq!(
            events[3],
            Event::Item(Item::Field {
                key: "A16SN".into(),
                value: "x=y".into()
            })
        );
        assert_eq!(
            items(&events),
            [
                "ACH17= ",
                "L3=0 ",
                "AQFILE=Chris  _Focal Clear ",
                "A16SN=x=y"
            ]
        );
    }

    #[test]
    fn waits_for_the_nul() {
        let mut parser = Parser::new();
        assert!(parser.feed(b"PWR=O").is_empty());
        assert!(parser.has_partial());
        assert_eq!(items(&parser.feed(b"N\0")), ["PWR=ON"]);
    }

    #[test]
    fn a_cr_inside_an_item_is_kept_and_a_lone_cr_is_no_packet_end() {
        let mut parser = Parser::new();
        assert_eq!(items(&parser.feed(b"\rX=a\rb\0")), ["X=a\rb"]);
    }
}
