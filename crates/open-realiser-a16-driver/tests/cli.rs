// SPDX-License-Identifier: MIT
//! The command lines of the binaries, and the Python checks of the repository: the
//! generated profile table and upmixer configuration are current, the driver finds its
//! configuration file, and the static DSP processes stereo as the upmixer graph does. None
//! of them needs an A16 or a PipeWire server.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn bin(name: &str) -> Command {
    Command::new(match name {
        "a16-driver" => env!("CARGO_BIN_EXE_a16-driver"),
        "a16-usb" => env!("CARGO_BIN_EXE_a16-usb"),
        _ => env!("CARGO_BIN_EXE_a16-static-dsp"),
    })
}

fn run(command: &mut Command) -> Output {
    command.output().expect("the binary runs")
}

/// The repository root.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Run a Python check of the repository: `python3`, or `$PYTHON`.
fn python(script: &str, args: &[&str]) {
    let python = std::env::var_os("PYTHON").unwrap_or_else(|| "python3".into());
    let output = run(Command::new(python)
        .arg("-I")
        .arg(root().join(script))
        .args(args)
        .env("PIPEWIRE_REMOTE", "offline-no-server"));
    assert!(
        output.status.success(),
        "{script}: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn driver_help() {
    let output = run(bin("a16-driver").arg("--help"));
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("a16-driver [--seconds 1..86400]"));
}

#[test]
fn driver_lists_profiles() {
    let output = run(bin("a16-driver").arg("--list-profiles"));
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("custom-2:"));
}

#[test]
fn driver_rejects_an_unknown_profile() {
    let output = run(bin("a16-driver").args(["--profile", "invalid"]));
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn usb_help() {
    let output = run(bin("a16-usb").arg("--help"));
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("a16-usb inspect"));
}

#[test]
fn usb_rejects_an_unbounded_stream() {
    let output = run(bin("a16-usb").arg("stream"));
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn config_paths() {
    python(
        "tools/libusb-harness/test_config.py",
        &[env!("CARGO_BIN_EXE_a16-driver")],
    );
}

#[test]
fn profile_catalog_is_current() {
    python("tools/libusb-harness/layouts/generate.py", &["--check"]);
}

#[test]
fn upmix_config_is_current() {
    python("tools/pipewire-upmix/generate.py", &["--check"]);
}

#[test]
fn static_upmix() {
    let conf = root().join("tools/pipewire-upmix/a16-upmix.conf");
    python(
        "tools/pipewire-upmix/test-static.py",
        &[
            env!("CARGO_BIN_EXE_a16-static-dsp"),
            conf.to_str().expect("a UTF-8 path"),
        ],
    );
}
