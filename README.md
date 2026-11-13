# open-realiser-a16-driver

A Linux userspace USB audio driver for the **Smyth Realiser A16**, with PipeWire
playback/recording and an optional surround upmixer. It uses libusb rather than a
custom kernel module.

**Credit to [Ozzy](https://github.com/mischa85/Ozzy) and its contributors for
reverse-engineering the Ploytec USB audio protocol and implementing the protocol
engine this driver builds on.** This project's additions focus on A16 userspace
integration, PipeWire playback/recording, and upmixing.

## Build

Build the published package with `nix build github:clhodapp/open-realiser-a16-driver`.
From a checkout:

```sh
nix build path:.
```

The package is `open-realiser-a16-driver`; `nix run path:. -- --help` opens the
driver help. The flake's `overlays.default` adds it to a package set as
`pkgs.open-realiser-a16-driver.open-realiser-a16-driver`. Both x86-64 and ARM64 Linux package definitions are provided.

Without Nix, install a C11 compiler, CMake, pkg-config, libusb, PipeWire development
headers, libsamplerate, and Python 3, then:

```sh
cmake -S . -B build-userspace -DCMAKE_BUILD_TYPE=Release
cmake --build build-userspace -j
ctest --test-dir build-userspace --output-on-failure
cmake --install build-userspace --prefix "$HOME/.local"
```

## Use the A16 as a soundcard

```sh
./result/bin/a16-driver --list-profiles
./result/bin/a16-driver --profile custom-2
```

Run the driver in your desktop session with access to the A16 USB device and
PipeWire. **Match the A16 PCM room to the selected profile's USB slot map.**
`custom-2` is the 16-slot layout used by the upmixer; the driver otherwise defaults
to the 12-channel `7.1.4t` compatibility profile. Selecting a profile does not
configure the Realiser's room, volume or input.

The driver exposes playback and a stereo USB return at 48 kHz. Select the desired
playback device in your player or desktop audio settings. Stop with Ctrl-C.
Automatic reconnect is not implemented; restart after a disconnection.

See [driver configuration and channel maps](tools/libusb-harness/PIPEWIRE.md) for
profile selection, missing speakers, USB permissions and capture routing.

## Optional live upmixer

With the driver running in `custom-2`, start this in another terminal:

```sh
./result/bin/a16-upmix
# Or: nix run path:.#upmix
```

Select the stereo, 5.1 or 7.1 upmix device to match your source. These produce
7.1.4 output using PipeWire's stereo PSD stage and configurable ambience/height
feeds. Original 5.1/7.1 channels, including LFE, pass through unchanged inside the
graph. Direct A16 playback remains available for bypass and native multichannel
PCM. The launcher does not select a default device or install an autostart service.

This is matrix/ambience upmixing, not Dolby/DTS decoding or object reconstruction.
The supplied height mapping targets the documented A16 Custom #2 room. Stereo
adds no synthetic LFE; bass management remains on the A16. Read the
[upmixer guide](tools/pipewire-upmix/README.md) for tuning, headroom and mapping.
Stop the upmixer with Ctrl-C independently of the driver.

## Included tools

| Command | Purpose |
| --- | --- |
| `a16-driver` | Live PipeWire soundcard: playback plus stereo USB return |
| `a16-upmix` | Live stereo/5.1/7.1 to 7.1.4 upmix devices |
| `a16-usb` | USB inspection and bounded WAV playback/capture diagnostics |
| `a16-static-dsp` | Offline stereo-to-16-slot Custom #2 PCM processing |
| `a16-play-static` | Play a prepared Custom #2 RF64 file directly to the A16 |

## Scope and provenance

This package focuses on the A16 userspace implementation. It is experimental:
local hardware checks and offline tests do not establish every room, channel,
rate or long-running workload. The live bridge uses 48 kHz; the USB diagnostic
streamer supports 44.1/48/88.2/96 kHz. Tests run offline and do not need an A16.
Historical validation details remain in the [USB harness notes](tools/libusb-harness/README.md).

The Ploytec reverse-engineering work and underlying protocol engine come from
Ozzy and its contributors. See the [original project documentation](README.ozzy.md)
and [MIT license](LICENSE).
