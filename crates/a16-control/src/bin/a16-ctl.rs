// SPDX-License-Identifier: MIT
//! `a16-ctl`: status, monitoring and commands through the TCP command server of the A16.

use std::env;
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use a16_control::speaker;
use a16_control::{Command, Connection, Endpoint, Item, Packet, Power, PowerState, Status, User};

const USAGE: &str = "\
a16-ctl [--host HOST --port PORT | --command CMD] SUBCOMMAND

Talks to the TCP command server of a Smyth Realiser A16 (Settings > Network on the A16).

Subcommands:
  status          power, firmware, users A and B, assignments and active speakers
  watch           print every item the A16 sends, with a UTC timestamp, until it
                  closes the connection
  send COMMAND    send a named command and print its reply
  raw HEX         send any code, 00 to ff, and print the reply

Commands for send:
  key NAME                 an IR remote key: power menu back enter cancel adj+ adj-
                           up down left right 0-9 * # cal spk hp c l r lw rw ls rs
                           lb rb lfe oh all headphone speaker a-vol+ a-vol- b-vol+
                           b-vol- mute-a mute-b test ht pa pb
  input NAME               earc hdmi-1 hdmi-2 hdmi-3 hdmi-4 usb line stereo coaxial
                           optical
  power on|standby|status
  preset a|b 1-16
  volume a|b [up|down]     without a direction, request the volume
  mute a|b                 toggle the mute of a user
  info a|b                 request the info of a user
  angle a|b                request the head-tracker angle of a user
  speaker ID|DESIGNATOR    select speaker 1-50, such as 5 or Ls: solo or mute it by
                           the ALL mode
  assignments | firmware | levels | reset-levels | visibility | active

Connection:
  --host HOST, --port PORT  TCP to the A16; defaults A16_HOST, A16_PORT
  --command CMD             run CMD with sh -c and speak the protocol on its stdin
                            and stdout; default A16_COMMAND
  The command line overrides the environment. A host and a command together are
  an error.

Exit status: 0, 1 on a failure or a missing reply, 2 on a bad command line.";

struct Options {
    host: Option<String>,
    port: Option<String>,
    command: Option<String>,
    words: Vec<String>,
}

fn parse_args(args: &[String]) -> Result<Option<Options>, String> {
    let mut o = Options {
        host: None,
        port: None,
        command: None,
        words: Vec::new(),
    };
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let slot = match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--host" => &mut o.host,
            "--port" => &mut o.port,
            "--command" => &mut o.command,
            _ if arg.starts_with("--") => return Err(format!("unknown option {arg}")),
            _ => {
                o.words.push(arg.clone());
                continue;
            }
        };
        *slot = Some(args.next().ok_or(format!("{arg} needs a value"))?.clone());
    }
    Ok(Some(o))
}

fn endpoint(o: &Options) -> Result<Endpoint, String> {
    let var = |name| env::var(name).ok().filter(|v: &String| !v.is_empty());
    let port = o.port.clone().or_else(|| var("A16_PORT"));
    let (host, command) = if o.host.is_some() || o.command.is_some() {
        (o.host.clone(), o.command.clone())
    } else {
        (var("A16_HOST"), var("A16_COMMAND"))
    };
    match (host, command) {
        (Some(_), Some(_)) => Err("give a host or a command, not both".into()),
        (None, Some(command)) => Ok(Endpoint::Command {
            program: "sh".into(),
            args: vec!["-c".into(), command],
        }),
        (Some(host), None) => {
            let port = port.ok_or("a host needs --port or A16_PORT")?;
            let port = port.parse().map_err(|_| format!("bad port {port}"))?;
            Ok(Endpoint::Tcp { host, port })
        }
        (None, None) => Err("no A16: give --host and --port, or --command".into()),
    }
}

/// An ISO 8601 UTC time with milliseconds.
fn timestamp(time: SystemTime) -> String {
    let since = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = since.as_secs();
    let (days, rest) = (seconds / 86400, seconds % 86400);
    // Howard Hinnant's civil_from_days.
    let z = days as i64 + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rest / 3600,
        rest / 60 % 60,
        rest % 60,
        since.subsec_millis()
    )
}

/// The item as text, with control characters escaped.
fn show(item: &Item) -> String {
    item.to_string().escape_default().to_string()
}

fn or_dash(text: Option<&str>) -> &str {
    match text.map(str::trim) {
        Some("") | None => "-",
        Some(text) => text,
    }
}

fn line(label: &str, value: &str) {
    println!("{label:<24}{value}");
}

fn speaker_list(ids: &[u8]) -> String {
    let names: Vec<String> = ids
        .iter()
        .map(|&id| match speaker::by_id(id) {
            Some(s) => format!("{}({id})", s.designator),
            None => id.to_string(),
        })
        .collect();
    if names.is_empty() {
        "-".into()
    } else {
        names.join(" ")
    }
}

fn print_status(s: &Status) {
    let power = s.power.map(|p| match p {
        PowerState::On => "on",
        PowerState::Standby => "standby",
        PowerState::Boot => "booting",
    });
    line("power", or_dash(power));
    for (field, value) in &s.firmware {
        line(field.label(), or_dash(Some(value)));
    }
    line("mode", or_dash(s.mode.as_deref()));
    line("all", or_dash(s.all.as_deref()));
    line("test", or_dash(s.test.as_deref()));
    for (field, value) in &s.system {
        line(field.label(), or_dash(Some(value)));
    }
    line("active speakers", &speaker_list(&s.active));
    if !s.muted.is_empty() {
        line("muted speakers", &speaker_list(&s.muted));
    }
    for user in [User::A, User::B] {
        let u = s.user(user);
        println!();
        println!("User {user}");
        for (field, value) in &u.text {
            line(&format!("  {}", field.label()), or_dash(Some(value)));
        }
        let number = |n: Option<u8>| n.map_or("-".into(), |n| n.to_string());
        line("  preset", &number(u.preset));
        line("  volume", &number(u.volume));
        if let Some(muted) = u.mute {
            line("  mute", if muted { "muted" } else { "off" });
        }
        let channels: Vec<String> = u
            .assignments
            .iter()
            .filter(|(_, d)| !d.is_empty())
            .map(|(n, d)| format!("{n}:{d}"))
            .collect();
        line("  channels", or_dash(Some(&channels.join(" "))));
    }
    if !s.unknown.is_empty() {
        println!();
        println!("Other items");
        for item in &s.unknown {
            println!("  {}", show(item));
        }
    }
}

fn status(connection: &mut Connection) -> ExitCode {
    let mut packets = Vec::new();
    let mut missing = Vec::new();
    for command in [
        Command::Power(Power::Status),
        Command::Firmware,
        Command::Info(User::A),
        Command::Info(User::B),
        Command::Assignments,
        Command::ActiveSpeakers,
    ] {
        match connection.request(command) {
            Ok(Some(packet)) => packets.push(packet),
            Ok(None) => missing.push(format!("{:02x}", command.code())),
            Err(e) => {
                eprintln!("a16-ctl: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    // Items from the IR remote or the front panel may arrive between the replies; the state
    // is the fold of all of them in arrival order.
    packets.extend(connection.take_unsolicited());
    packets.sort_by_key(|p| p.received);
    let mut s = Status::default();
    for packet in &packets {
        packet.items.iter().for_each(|item| s.apply(item));
    }
    print_status(&s);
    if missing.is_empty() {
        ExitCode::SUCCESS
    } else {
        eprintln!("a16-ctl: no reply to {}", missing.join(", "));
        ExitCode::FAILURE
    }
}

fn watch(connection: &mut Connection) -> ExitCode {
    loop {
        match connection.next_packet(Duration::from_secs(3600)) {
            Ok(Some(packet)) => {
                let time = timestamp(packet.received);
                for item in &packet.items {
                    println!("{time} {}", show(item));
                }
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!("a16-ctl: {e}");
                return if e.kind() == std::io::ErrorKind::UnexpectedEof {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::FAILURE
                };
            }
        }
    }
}

fn print_packet(prefix: &str, packet: &Packet) {
    for item in &packet.items {
        println!("{prefix}{}", show(item));
    }
}

fn send(connection: &mut Connection, command: Command) -> ExitCode {
    let reply = connection.request(command);
    for packet in connection.take_unsolicited() {
        print_packet("unsolicited: ", &packet);
    }
    match reply {
        Ok(Some(packet)) => {
            print_packet("", &packet);
            ExitCode::SUCCESS
        }
        Ok(None) => {
            eprintln!(
                "a16-ctl: no reply to {:02x} within {} s",
                command.code(),
                connection.timeout.as_secs_f32()
            );
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("a16-ctl: {e}");
            ExitCode::FAILURE
        }
    }
}

enum Action {
    Status,
    Watch,
    Send(Command),
}

fn action(words: &[String]) -> Result<Action, String> {
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["status"] => Ok(Action::Status),
        ["watch"] => Ok(Action::Watch),
        ["send", rest @ ..] => Command::parse(rest).map(Action::Send),
        ["raw", hex] => {
            let code = (hex.len() <= 2)
                .then(|| u8::from_str_radix(hex, 16).ok())
                .flatten()
                .ok_or(format!("raw takes a code from 00 to ff, not {hex}"))?;
            Ok(Action::Send(Command::from_code(code)))
        }
        [] => Err("no subcommand".into()),
        _ => Err(format!("unknown subcommand: {}", words.join(" "))),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let parsed = parse_args(&args).and_then(|o| match o {
        None => Ok(None),
        Some(o) => Ok(Some((action(&o.words)?, endpoint(&o)?))),
    });
    let (action, endpoint) = match parsed {
        Ok(Some(parsed)) => parsed,
        Ok(None) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("a16-ctl: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let mut connection = match Connection::open(&endpoint) {
        Ok(connection) => connection,
        Err(e) => {
            eprintln!("a16-ctl: cannot connect to the A16: {e}");
            return ExitCode::FAILURE;
        }
    };
    match action {
        Action::Status => status(&mut connection),
        Action::Watch => watch(&mut connection),
        Action::Send(command) => send(&mut connection, command),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_utc_timestamps() {
        let at = |s, ms| UNIX_EPOCH + Duration::from_secs(s) + Duration::from_millis(ms);
        assert_eq!(timestamp(at(0, 0)), "1970-01-01T00:00:00.000Z");
        assert_eq!(timestamp(at(951_782_400, 5)), "2000-02-29T00:00:00.005Z");
        assert_eq!(
            timestamp(at(1_794_754_803, 123)),
            "2026-11-15T15:00:03.123Z"
        );
    }

    #[test]
    fn raw_codes_decode_to_commands() {
        let raw = |hex: &str| match action(&["raw".into(), hex.into()]) {
            Ok(Action::Send(command)) => Some(command),
            _ => None,
        };
        assert_eq!(raw("2E"), Some(Command::Power(Power::Status)));
        assert_eq!(raw("1"), Some(Command::Raw(1)));
        assert_eq!(raw("100"), None);
        assert_eq!(raw("zz"), None);
    }
}
