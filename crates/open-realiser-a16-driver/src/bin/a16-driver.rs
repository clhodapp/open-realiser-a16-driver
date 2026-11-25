// SPDX-License-Identifier: MIT
//! `a16-driver`: the A16 as a PipeWire soundcard, a sink of the selected profile and a
//! stereo source of the USB return.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use a16_engine::Engine;
use open_realiser_a16_driver::bridge::{AudioPath, Bridge, Shared};
use open_realiser_a16_driver::bus::{UsbBus, engine_logger, stderr_log};
use open_realiser_a16_driver::libusb::LibUsb;
use open_realiser_a16_driver::nodes::PipeWire;
use open_realiser_a16_driver::notify;
use open_realiser_a16_driver::profiles::{self, Missing};
use open_realiser_a16_driver::realtime;
use open_realiser_a16_driver::strerror;

const HELP: &str = "a16-driver [--seconds 1..86400] [--detach] [--profile NAME] [--list-profiles] \
[--config FILE] [--missing-speakers Chr,...]\n48 kHz, selectable layout sink + stereo source. \
Does not change default routing.\nRuns until SIGINT/SIGTERM unless --seconds is supplied.";

static STOPPING: AtomicBool = AtomicBool::new(false);

extern "C" fn stop_signal(_signal: libc::c_int) {
    STOPPING.store(true, Ordering::SeqCst);
}

/// A decimal count as `strtoul` reads it, for an option that takes 1 to `max`.
fn count(text: &OsStr, max: u64) -> Option<u64> {
    let bytes = text.as_bytes();
    let start = bytes
        .iter()
        .position(|&c| !(c == b' ' || (b'\t'..=b'\r').contains(&c)))
        .unwrap_or(bytes.len());
    let mut digits = &bytes[start..];
    match digits.first() {
        Some(b'+') => digits = &digits[1..],
        // A negated count is 0 or wraps past any bound
        Some(b'-') => return None,
        _ => {}
    }
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let value: u64 = std::str::from_utf8(digits).ok()?.parse().ok()?;
    (1..=max).contains(&value).then_some(value)
}

struct Options {
    seconds: u64,
    detach: bool,
    profile: usize,
    list_profiles: bool,
    config: Option<OsString>,
    missing: Option<OsString>,
}

/// The options, or the exit status of `--help` or a rejected argument.
fn parse(args: &[OsString]) -> Result<Options, i32> {
    let mut o = Options {
        seconds: 0,
        detach: false,
        profile: 0,
        list_profiles: false,
        config: None,
        missing: None,
    };
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_bytes();
        let value = args.get(i + 1);
        match (arg, value) {
            (b"--help", _) => {
                println!("{HELP}");
                return Err(0);
            }
            (b"--list-profiles", _) => o.list_profiles = true,
            (b"--config", Some(value)) => {
                o.config = Some(value.clone());
                i += 1;
            }
            (b"--missing-speakers", Some(value)) => {
                o.missing = Some(value.clone());
                i += 1;
            }
            (b"--detach", _) => o.detach = true,
            (b"--profile", Some(value)) => {
                i += 1;
                let found = value.to_str().and_then(profiles::find);
                match found {
                    Some(p) if profiles::all()[p].available => o.profile = p,
                    _ => return Err(2),
                }
            }
            (b"--seconds", Some(value)) => {
                i += 1;
                o.seconds = count(value, 86400).ok_or(2)?;
            }
            _ => return Err(2),
        }
        i += 1;
    }
    Ok(o)
}

/// The configuration file a user keeps: under `$XDG_CONFIG_HOME`, else `~/.config`.
fn default_config() -> Option<PathBuf> {
    let set = |name| std::env::var_os(name).filter(|v| !v.is_empty());
    let path = if let Some(xdg) = set("XDG_CONFIG_HOME") {
        PathBuf::from(xdg).join("open-realiser-a16-driver/config.json")
    } else {
        PathBuf::from(set("HOME")?).join(".config/open-realiser-a16-driver/config.json")
    };
    (path.as_os_str().len() < libc::PATH_MAX as usize).then_some(path)
}

fn main() {
    let status = run();
    // A driver that exits before the A16 was ready, as when no A16 is attached, still
    // reports readiness, so systemd records a start followed by this exit status
    notify::ready();
    std::process::exit(status);
}

fn run() -> i32 {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let o = match parse(&args) {
        Ok(o) => o,
        Err(status) => return status,
    };
    let explicit = o.config.is_some();
    let Some(config) = o.config.clone().map(PathBuf::from).or_else(default_config) else {
        return 2;
    };
    let mut missing = Missing::default();
    if !missing.load(&config, !explicit) {
        eprintln!("Invalid or unreadable A16 config: {}", config.display());
        return 2;
    }
    if let Some(list) = &o.missing {
        missing = Missing::default();
        if !list.to_str().is_some_and(|list| missing.parse(list)) {
            eprintln!(
                "Invalid missing speaker identity list: {}",
                list.to_string_lossy()
            );
            return 2;
        }
    }
    // SAFETY: the handler only stores to an atomic
    unsafe {
        let handler = stop_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
        libc::signal(libc::SIGINT, handler);
        libc::signal(libc::SIGTERM, handler);
    }
    let Some(effective) = profiles::effective(&missing) else {
        return 2;
    };
    if o.list_profiles {
        for p in &effective {
            println!(
                "{}: {} ({} channels)\n  Speakers: {}\n  Required A16 room: {}",
                p.name,
                p.description,
                p.channels(),
                p.speakers,
                p.room
            );
        }
        return 0;
    }
    if !effective[o.profile].available {
        eprintln!("Profile has no available speakers");
        return 2;
    }
    if !missing.text().is_empty() {
        eprintln!("User missing speakers: {}", missing.text());
    }
    let shared = Arc::new(Shared::new(effective, o.profile));
    let result = session(&shared, &o, &missing);
    eprintln!("PipeWire/USB shutdown result={result}");
    result
}

/// PipeWire, then the A16 until the driver ends: the exit status.
fn session(shared: &Arc<Shared>, o: &Options, missing: &Missing) -> i32 {
    let Some(audio) = AudioPath::new(Arc::clone(shared), stderr_log()) else {
        return 1;
    };
    // Verify daemon connectivity before touching the USB device
    let pipewire = match PipeWire::open(Arc::clone(shared), o.profile, missing.text()) {
        Ok(pipewire) => pipewire,
        Err(error) => {
            eprintln!(
                "Cannot connect to PipeWire: {}",
                strerror(error.0.raw_os_error().unwrap_or(0))
            );
            return 1;
        }
    };
    let Ok(mut usb) = LibUsb::new() else {
        return 1;
    };
    if !usb.register_hotplug() {
        eprintln!(
            "No USB hotplug events; an idle driver notices the A16 leaving only when it streams"
        );
    }
    // This thread runs the USB side from here on. Threads inherit the scheduling policy
    // of the thread that creates them, so libusb's threads, started above, stay at normal
    // priority.
    match realtime::make_current_thread_realtime() {
        Ok(priority) => eprintln!("USB thread realtime at priority {priority} through rtkit"),
        Err(error) => eprintln!("USB thread at normal priority: {error}"),
    }
    let mut engine = Engine::new();
    engine.set_logger(engine_logger(|line| eprintln!("{line}")));
    let bus = UsbBus::new(usb, None, stderr_log());
    let mut bridge = Bridge::new(
        bus,
        engine,
        Arc::clone(shared),
        audio,
        o.detach,
        stderr_log(),
    );
    let mut pipewire = Some(pipewire);
    // Stop the PipeWire callbacks before the USB side closes; USB cancellation stays on
    // this thread
    bridge.drive(&STOPPING, o.seconds, || {
        if let Some(pipewire) = pipewire.take() {
            pipewire.close();
        }
    })
}
