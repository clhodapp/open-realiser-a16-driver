# Development

This project builds a Linux userspace driver for the Smyth Realiser A16 on
Ozzy's USB protocol engine. The MIT license, upstream history and upstream
README are retained. See [README.ozzy.md](../../README.ozzy.md) for that history.

The driver is a Cargo workspace of two crates:

- `crates/a16-engine`: the USB protocol engine, a Rust port of Ozzy's C engine,
  without dependencies outside std. `tests/a16_trace.rs` checks a trace of the
  engine on a fake bus against the SHA-256 of the trace the C engine produced.
- `crates/open-realiser-a16-driver`: the libusb bus (`bus.rs`, with the libusb
  calls behind the `Usb` trait of `usb.rs` and made in `libusb.rs`), the USB side
  of the PipeWire bridge with its recovery (`bridge.rs`), the PipeWire card and
  streams (`nodes.rs`), the queues and level check (`live.rs`), the profiles
  (`profiles.rs`, with the table `tools/libusb-harness/layouts/generate.py`
  writes), and the binaries `a16-driver`, `a16-usb` and `a16-static-dsp`.

`tools/libusb-harness/` keeps the profile data and generator and the harness
notes; `tools/pipewire-upmix/` keeps the upmixer configuration, its generator and
the launchers.

`cargo test --workspace` runs every test: the engine's, the bus on scripted libusb
calls, the bridge's USB lifecycle on a fake A16 and a fake clock
(`tests/lifecycle.rs`), the command lines, and the Python checks of `tools/`
(generated files current, configuration discovery, static upmixing). They run
without USB hardware or a PipeWire server. `nix flake check` builds the package,
which runs them in the sandbox. The GitHub workflow runs these checks on x86-64
and ARM64 Linux.

The A16 drops off USB during its boot if playback is running when it leaves
its boot splash, about 43.5 s after a power-up;
[boot-usb-drop.md](boot-usb-drop.md) records that behavior and how to recover
from it.

For a native build and hardware usage, follow the [README](../../README.md).
Hardware validation runs require explicit coordination with the device owner,
since changing USB interfaces or restarting the driver interrupts playback.
