// SPDX-License-Identifier: MIT
//! `a16-usb`: the A16's USB descriptors, and bounded streaming with WAV playback and capture,
//! for diagnostics.

use std::ffi::OsString;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::os::unix::ffi::OsStrExt as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use a16_engine::{Direction, Engine, PcmRing, SampleRate};
use open_realiser_a16_driver::bus::{UsbBus, engine_logger, stderr_log};
use open_realiser_a16_driver::libusb::{LibUsb, config_descriptor};
use open_realiser_a16_driver::usb::{Usb, UsbError};
use open_realiser_a16_driver::{log, perror, wav};
use rusb::UsbContext as _;
use rusb::ffi;

const USAGE: &str = "a16-usb inspect [--device BUS:ADDRESS]
a16-usb stream --seconds 1..60 [--device BUS:ADDRESS] [--detach] [--rate 44100|48000|88200|96000]
                   [--play input.wav] [--capture output.wav] [--log trace.txt]
A16 only (0a4a:5a16), 44100/48000/88200/96000 Hz (default 48000), 16 out / 2 in, 24-bit PCM.
inspect reads descriptors without opening/claiming interfaces.
stream changes device state; silence unless --play is given. WAV playback is
one-shot, requires 16 channels at the selected rate, and pads remaining time with silence.
--detach permits temporary kernel driver detachment; reattached on clean exit.";

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_signal: libc::c_int) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

/// A decimal number up to `max`, as `strtoul` reads it, not starting with a minus.
fn number(text: &[u8], max: u32) -> Option<u32> {
    if text.is_empty() || text[0] == b'-' {
        return None;
    }
    let start = text
        .iter()
        .position(|&c| !(c == b' ' || (b'\t'..=b'\r').contains(&c)))
        .unwrap_or(text.len());
    let mut digits = &text[start..];
    match digits.first() {
        Some(b'+') => digits = &digits[1..],
        // A negated number is 0 or wraps past the bound
        Some(b'-') => return None,
        _ => {}
    }
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let value: u64 = std::str::from_utf8(digits).ok()?.parse().ok()?;
    (value <= u64::from(max)).then_some(value as u32)
}

struct Options {
    stream: bool,
    detach: bool,
    device: Option<(u32, u32)>,
    rate: u32,
    seconds: u32,
    play: Option<OsString>,
    capture: Option<OsString>,
    log: Option<OsString>,
}

fn usage() {
    println!("{USAGE}");
}

/// The options, or None for a rejected command line.
fn parse(args: &[OsString]) -> Option<Options> {
    let mut o = Options {
        stream: args[0].as_bytes() == b"stream",
        detach: false,
        device: None,
        rate: 48000,
        seconds: 0,
        play: None,
        capture: None,
        log: None,
    };
    let stream = o.stream;
    let mut i = 1;
    while i < args.len() {
        let arg = args[i].as_bytes();
        let value = args.get(i + 1);
        match (arg, value) {
            (b"--detach", _) if stream => o.detach = true,
            (b"--device", Some(value)) => {
                i += 1;
                let value = value.as_bytes();
                if value.len() >= 32 {
                    return None;
                }
                let colon = value.iter().position(|&c| c == b':')?;
                let bus = number(&value[..colon], 255)?;
                let address = number(&value[colon + 1..], 127).filter(|&a| a != 0)?;
                o.device = Some((bus, address));
            }
            (b"--seconds", Some(value)) if stream => {
                i += 1;
                o.seconds = number(value.as_bytes(), 60).filter(|&s| s != 0)?;
            }
            (b"--rate", Some(value)) if stream => {
                i += 1;
                o.rate = number(value.as_bytes(), 96000)
                    .filter(|r| [44100, 48000, 88200, 96000].contains(r))?;
            }
            (b"--play", Some(value)) if stream => {
                i += 1;
                o.play = Some(value.clone());
            }
            (b"--capture", Some(value)) if stream => {
                i += 1;
                o.capture = Some(value.clone());
            }
            (b"--log", Some(value)) if stream => {
                i += 1;
                o.log = Some(value.clone());
            }
            _ => return None,
        }
        i += 1;
    }
    if stream && o.seconds == 0 {
        return None;
    }
    Some(o)
}

fn main() {
    let status = run();
    log::flush();
    std::process::exit(status);
}

fn run() -> i32 {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if args.len() == 1 && args[0].as_bytes() == b"--help" {
        usage();
        return 0;
    }
    if args.is_empty() || !matches!(args[0].as_bytes(), b"inspect" | b"stream") {
        usage();
        return 2;
    }
    let Some(o) = parse(&args) else {
        usage();
        return 2;
    };
    // Validate input files before initializing USB, much less claiming the device
    let frames = (o.seconds + 2) * o.rate;
    let (mut out, mut input) = if o.stream {
        (
            vec![0u8; frames as usize * 48],
            vec![0u8; frames as usize * 6],
        )
    } else {
        (Vec::new(), Vec::new())
    };
    let mut play_frames = 0;
    if let Some(play) = &o.play {
        let capacity = (o.seconds * o.rate) as usize * 48;
        match wav::read(&PathBuf::from(play), o.rate, 16, &mut out[..capacity]) {
            Some(n) => play_frames = n,
            None => {
                eprintln!(
                    "playback must be a valid {}-Hz 16-channel 24-bit PCM WAV no longer than --seconds",
                    o.rate
                );
                return 2;
            }
        }
    }
    let mut record = None;
    if let Some(capture) = &o.capture {
        match File::create_new(capture) {
            Ok(file) => record = Some(file),
            Err(error) => {
                eprintln!("{}", perror(&capture.to_string_lossy(), &error));
                return 1;
            }
        }
    }
    let mut trace = None;
    if let Some(log) = &o.log {
        match File::create_new(log) {
            Ok(file) => trace = Some(file),
            Err(error) => {
                eprintln!("{}", perror(&log.to_string_lossy(), &error));
                return 1;
            }
        }
    }
    let mut usb = match LibUsb::new() {
        Ok(usb) => usb,
        Err(error) => {
            eprintln!("libusb init: {}", UsbError::from(error));
            return 1;
        }
    };
    let devices = match usb.context().devices() {
        Ok(devices) => devices,
        Err(error) => {
            eprintln!("list: {}", UsbError::from(error));
            return 1;
        }
    };
    let mut chosen = None;
    let mut matches = 0;
    for device in devices.iter() {
        let raw = device.as_raw();
        // SAFETY: a listed device, referenced by the list
        let (d, bus, address, speed) = unsafe {
            let mut d: ffi::libusb_device_descriptor = std::mem::zeroed();
            if ffi::libusb_get_device_descriptor(raw, &mut d) != 0 {
                continue;
            }
            (
                d,
                u32::from(ffi::libusb_get_bus_number(raw)),
                u32::from(ffi::libusb_get_device_address(raw)),
                ffi::libusb_get_device_speed(raw),
            )
        };
        if d.idVendor != a16_engine::VENDOR_ID || d.idProduct != a16_engine::PRODUCT_ID {
            continue;
        }
        if o.device.is_some_and(|selected| selected != (bus, address)) {
            continue;
        }
        matches += 1;
        describe(raw, &d, bus, address, speed);
        chosen = Some((device, speed));
    }
    let _ = std::io::stdout().flush();
    if !o.stream {
        if matches == 0 {
            println!("No matching A16 found.");
        }
        return 0;
    }
    if matches != 1 {
        eprintln!("expected one A16, found {matches}; select --device BUS:ADDRESS");
        return 1;
    }
    let Some((device, speed)) = chosen else {
        return 1;
    };
    if speed != ffi::constants::LIBUSB_SPEED_HIGH {
        eprintln!("A16 must be at USB high speed");
        return 1;
    }
    if let Err(error) = usb.open_device(&device) {
        eprintln!("open A16: {error} (check USB device permissions)");
        return 1;
    }
    stream(
        usb,
        &o,
        frames,
        play_frames,
        out,
        &mut input,
        trace,
        &mut record,
    )
}

/// The descriptors of the A16 at `raw`, on stdout.
fn describe(
    raw: *mut ffi::libusb_device,
    d: &ffi::libusb_device_descriptor,
    bus: u32,
    address: u32,
    speed: i32,
) {
    println!(
        "device {bus}:{address} {:04x}:{:04x} bcdDevice={:04x} speed={speed} configurations={}",
        d.idVendor, d.idProduct, d.bcdDevice, d.bNumConfigurations
    );
    for c in 0..d.bNumConfigurations {
        // SAFETY: a listed device, referenced by the list
        let config = match unsafe { config_descriptor(raw, Some(c)) } {
            Ok(config) => config,
            Err(error) => {
                eprintln!("descriptor: {error}");
                continue;
            }
        };
        println!(
            " configuration {} interfaces={}",
            config.value, config.interfaces
        );
        for s in &config.settings {
            println!(
                "  interface {} alt={} class={:02x} subclass={:02x} protocol={:02x}",
                s.interface, s.alt_setting, s.class, s.subclass, s.protocol
            );
            for ep in &s.endpoints {
                println!(
                    "   endpoint {:02x} type={} maxPacket={} interval={}",
                    ep.address,
                    ep.attributes & 3,
                    ep.max_packet_size,
                    ep.interval
                );
            }
        }
    }
}

/// Stream on the open A16 for `o.seconds`: the exit status.
#[allow(clippy::too_many_arguments)]
fn stream(
    usb: LibUsb,
    o: &Options,
    frames: u32,
    play_frames: u32,
    out: Vec<u8>,
    input: &mut Vec<u8>,
    trace: Option<File>,
    record: &mut Option<File>,
) -> i32 {
    let trace = trace.map(|file| Box::new(BufWriter::new(file)) as Box<dyn Write>);
    let mut bus = UsbBus::new(usb, trace, stderr_log());
    let mut engine = Engine::new();
    engine.set_logger(engine_logger(log::line));
    let mut result = 1;
    let mut attached = false;
    if bus.configure(&mut engine, o.detach) {
        attached = true;
        engine.set_rate(SampleRate::from_hz(o.rate));
        engine.attach_pcm_ring(PcmRing::from_bytes(Direction::Out, out), 0);
        engine.attach_pcm_ring(PcmRing::from_bytes(Direction::In, std::mem::take(input)), 0);
        // SAFETY: the handler only stores to an atomic
        unsafe {
            let handler = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
            libc::signal(libc::SIGINT, handler);
            libc::signal(libc::SIGTERM, handler);
        }
        eprintln!(
            "Starting experimental A16 engine: {} Hz, {play_frames} playback frames, {} seconds",
            o.rate, o.seconds
        );
        if engine.start(&mut bus).is_ok() && !bus.failed {
            let start = bus.usb.now();
            let deadline = start + u64::from(o.seconds) * 1_000_000_000;
            let mut last = start;
            while !INTERRUPTED.load(Ordering::SeqCst) && !bus.failed && bus.usb.now() < deadline {
                let _ = bus.poll(&mut engine, Duration::from_millis(1), None);
                if bus.progress[0] >= u64::from(frames) || bus.progress[1] >= u64::from(frames) {
                    eprintln!("PCM exceeded bounded recording buffers");
                    bus.failed = true;
                    break;
                }
                if bus.usb.now() - last >= 1_000_000_000 {
                    eprintln!(
                        "PCM frames playback={} capture={}, pending={}",
                        bus.progress[0],
                        bus.progress[1],
                        bus.pending()
                    );
                    last = bus.usb.now();
                }
            }
            result = i32::from(bus.failed || bus.progress[0] == 0 || bus.progress[1] == 0);
        }
    }
    if attached {
        engine.stop(&mut bus);
    }
    if let Some(file) = record.as_mut() {
        let got = bus.progress[1].min(u64::from(o.seconds) * u64::from(o.rate));
        let ring = engine
            .detach_pcm_ring(Direction::In)
            .map(PcmRing::into_bytes)
            .unwrap_or_else(|| std::mem::take(input));
        if let Err(error) = wav::write(file, o.rate, 2, &ring, got as u32) {
            eprintln!("{}", perror("capture write", &error));
            result = 1;
        }
        eprintln!(
            "Captured {got} frames; USB transfers out={} in={}",
            bus.completed[0], bus.completed[1]
        );
    }
    bus.release();
    if bus.failed {
        result = 1;
    }
    eprintln!("Stream/cleanup result={result}");
    if bus.finish_trace().is_err() {
        result = 1;
    }
    result
}
