# Development

This project builds a Linux userspace driver for the Smyth Realiser A16 on
Ozzy's USB protocol engine. The MIT license, upstream history and upstream
README are retained. See [README.ozzy.md](../../README.ozzy.md) for that history.

The A16 bridge lives in `tools/libusb-harness/`, with PipeWire processing in
`tools/pipewire-upmix/`. Shared protocol code lives in `common/` and `engine/`.

Run `nix flake check` to build the package and execute its CTest suite and shared
engine host tests. Tests
cover fake-bus startup, USB transfer cleanup, profile generation, PipeWire
argument handling, live audio buffering and static upmixing. They run without
USB hardware or an active PipeWire server. The GitHub workflow runs these checks
on x86-64 and ARM64 Linux.

For a native build and hardware usage, follow the [README](../../README.md).
Hardware validation runs require explicit coordination with the device owner,
since changing USB interfaces or restarting the driver interrupts playback.
