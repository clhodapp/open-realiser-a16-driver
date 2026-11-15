// SPDX-License-Identifier: MIT
//! `a16-ctl` and the connection against a fake A16: a TCP listener that sends the connect
//! burst and answers requests with replies a real A16 sent, written in small pieces.

#[path = "../src/fixtures.rs"]
mod fixtures;

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command as Process, Output};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use a16_control::{Command, Connection, Endpoint, Item, Status, User};

const BURST: &[u8] = b"ALL=SOLO\0TEST=OFF\0MODE=SVS\0A00\0A01\0A02\0\rV00\0V01\0V02\0\r";

fn reply(code: &str) -> Option<&'static [u8]> {
    Some(match code {
        "2e" => fixtures::POWER,
        "64" => fixtures::FIRMWARE,
        "80" => fixtures::USER_A_INFO,
        "a0" => fixtures::USER_B_INFO,
        "37" => fixtures::ASSIGNMENTS,
        "83" => fixtures::VOLUME_A,
        "a3" => fixtures::VOLUME_B,
        "6a" => fixtures::LEVELS,
        "ae" => fixtures::VISIBILITY,
        "af" => fixtures::ACTIVE,
        "25" => b"IN=USB\0\r",
        "43" => b"KEY=43\0\r",
        _ => return None,
    })
}

/// Writes in pieces of 7 bytes, so that items and packets span reads.
fn write_slowly(stream: &mut TcpStream, bytes: &[u8]) {
    for piece in bytes.chunks(7) {
        stream.write_all(piece).unwrap();
        stream.flush().unwrap();
        thread::sleep(Duration::from_millis(1));
    }
}

struct FakeA16 {
    port: u16,
    received: Arc<Mutex<Vec<String>>>,
}

impl FakeA16 {
    /// Serves one connection. Before its reply to `80` it sends an unsolicited volume change,
    /// as with TCP slave response on.
    fn start() -> FakeA16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let received = Arc::new(Mutex::new(Vec::new()));
        let log = received.clone();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            write_slowly(&mut stream, BURST);
            let reader = BufReader::new(stream.try_clone().unwrap());
            for line in reader.lines() {
                let Ok(line) = line else { return };
                let code = line.trim_end_matches('\r').to_string();
                log.lock().unwrap().push(code.clone());
                if code == "80" {
                    write_slowly(&mut stream, b"VB=49\0\r");
                }
                if let Some(bytes) = reply(&code) {
                    write_slowly(&mut stream, bytes);
                }
            }
        });
        FakeA16 { port, received }
    }

    fn received(&self) -> Vec<String> {
        self.received.lock().unwrap().clone()
    }
}

fn ctl(args: &[&str]) -> Output {
    Process::new(env!("CARGO_BIN_EXE_a16-ctl"))
        .args(args)
        .env_remove("A16_HOST")
        .env_remove("A16_PORT")
        .env_remove("A16_COMMAND")
        .output()
        .unwrap()
}

fn ctl_tcp(fake: &FakeA16, args: &[&str]) -> (bool, String) {
    let port = fake.port.to_string();
    let mut all = vec!["--host", "127.0.0.1", "--port", &port];
    all.extend_from_slice(args);
    let output = ctl(&all);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success() || !stderr.is_empty());
    (
        output.status.success(),
        String::from_utf8(output.stdout).unwrap(),
    )
}

#[test]
fn requests_match_replies_and_keep_unsolicited_packets() {
    let fake = FakeA16::start();
    let mut connection = Connection::open(&Endpoint::Tcp {
        host: "127.0.0.1".into(),
        port: fake.port,
    })
    .unwrap();
    let info = connection.request(Command::Info(User::A)).unwrap().unwrap();
    assert_eq!(info.items[0], Item::parse("AUR=User 1"));
    assert_eq!(info.items.len(), 19);
    let unsolicited = connection.take_unsolicited();
    let items: Vec<String> = unsolicited
        .iter()
        .flat_map(|p| p.items.iter().map(Item::to_string))
        .collect();
    assert_eq!(
        items,
        [
            "ALL=SOLO", "TEST=OFF", "MODE=SVS", "A00", "A01", "A02", "V00", "V01", "V02", "VB=49"
        ]
    );
    let volume = connection
        .request(Command::VolumeLevel(User::B))
        .unwrap()
        .unwrap();
    assert_eq!(volume.items, [Item::parse("VB=50")]);
    let mut status = Status::default();
    let firmware = connection.request(Command::Firmware).unwrap().unwrap();
    firmware.items.iter().for_each(|i| status.apply(i));
    assert_eq!(status.firmware.len(), 5);
    assert!(connection.take_unsolicited().is_empty());
    assert_eq!(fake.received(), ["80", "a3", "64"]);
}

#[test]
fn status_prints_the_state() {
    let fake = FakeA16::start();
    let (ok, out) = ctl_tcp(&fake, &["status"]);
    assert!(ok, "{out}");
    assert_eq!(fake.received(), ["2e", "64", "80", "a0", "37", "af"]);
    for expected in [
        "power                   on\n",
        "A16 firmware            2.17 Jan 26 2024\n",
        "serial number           A162U2001#000304\n",
        "input                   USB\n",
        "mode                    SVS\n",
        "active speakers         L(1) R(2) C(3) SW(4) Lc(17) Rc(18)",
        "User A\n  user                  User 1\n",
        "  HPEQ file             Chris  _Focal Clear\n",
        "  preset                1\n",
        "  channels              1:L 2:R 3:C 4:SW 5:Lc",
        "User B\n  user                  User 2\n",
        "  volume                50\n",
    ] {
        assert!(out.contains(expected), "{expected:?} not in\n{out}");
    }
    assert!(!out.contains("Other items"), "{out}");
}

#[test]
fn send_writes_the_named_command() {
    let fake = FakeA16::start();
    let (ok, out) = ctl_tcp(&fake, &["send", "input", "usb"]);
    assert!(ok);
    assert!(out.ends_with("IN=USB\n"), "{out}");
    assert!(out.contains("unsolicited: MODE=SVS\n"), "{out}");
    assert_eq!(fake.received(), ["25"]);
}

#[test]
fn raw_sends_any_code() {
    let fake = FakeA16::start();
    let (ok, out) = ctl_tcp(&fake, &["raw", "43"]);
    assert!(ok);
    assert!(out.ends_with("KEY=43\n"), "{out}");
    let fake = FakeA16::start();
    let (ok, _) = ctl_tcp(&fake, &["raw", "01"]);
    assert!(ok, "the first packet answers a code with no function");
    assert_eq!(fake.received(), ["01"]);
}

#[test]
fn a_missing_reply_fails() {
    let fake = FakeA16::start();
    let (ok, _) = ctl_tcp(&fake, &["send", "key", "enter"]);
    assert!(!ok);
    assert_eq!(fake.received(), ["04"]);
}

#[test]
fn watch_prints_items_until_the_connection_closes() {
    let output = ctl(&[
        "--command",
        r"printf 'ALL=SOLO\000\000A00\000\r'; printf 'MODE=AV\000'",
        "watch",
    ]);
    assert!(output.status.success());
    let out = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 3, "{out}");
    for (line, item) in lines.iter().zip(["ALL=SOLO", "A00", "MODE=AV"]) {
        let (time, rest) = line.split_once(' ').unwrap();
        assert_eq!(rest, item);
        assert!(time.len() == 24 && time.ends_with('Z'), "{time}");
    }
}

#[test]
fn a_command_relays_the_protocol() {
    // The reply has no trailing CR, so the quiet gap ends the packet.
    let script = r"read code; [ $code = $(printf '83\r') ] && printf 'VA=50\000'; sleep 2";
    let output = ctl(&["--command", script, "send", "volume", "a"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "VA=50\n");
}

#[test]
fn bad_command_lines_exit_2() {
    for args in [
        &["status"][..],
        &["--host", "h", "--command", "c", "status"],
        &["--host", "h", "status"],
        &["--host", "h", "--port", "1", "send", "preset", "a", "17"],
        &["--host", "h", "--port", "1", "frob"],
        &["--bogus"],
    ] {
        assert_eq!(ctl(args).status.code(), Some(2), "{args:?}");
    }
    let help = ctl(&["--help"]);
    assert!(help.status.success());
    assert!(
        String::from_utf8(help.stdout)
            .unwrap()
            .contains("send COMMAND")
    );
}
