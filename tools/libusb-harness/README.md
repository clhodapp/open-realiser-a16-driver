# A16 userspace validation harness

`ozzy-libusb` runs Ozzy's existing protocol engine over asynchronous libusb
transfers on Linux. It needs no custom kernel module or system audio setup.
This checkout started at upstream commit
`e0345d95dc987382ccf3f83b58a57d949d85a40e`; the shared engine and codec sources
are unchanged. The new backend is intentionally restricted to the experimental
Smyth Realiser A16 profile (`0a4a:5a16`): 44,100 Hz, 16 playback channels,
2 capture channels. Silent duplex streaming has been validated against a
connected A16 at 44,100 Hz. A quiet known-signal test also returned the transmitted
frequency in both capture channels. Audible playback, speaker mapping and the
exact stereo capture routing have not yet been validated.

Repeating the identical -60 dBFS 440 Hz channel-1 probe after the user increased
the Realiser's User A volume raised the captured tone from -99.858 to -89.858
dBFS on the left (+10.000 dB) and from -110.164 to -100.173 dBFS on the right
(+9.991 dB). This demonstrates that User A's volume affects this USB stereo
return under the tested configuration. The actual front-panel settings were
not recorded; this does not identify every routing stage or User B behavior.

The inspected unit reports `bcdDevice=0100`, USB high speed, and these endpoints
at alternate setting 1 (both interfaces are vendor-specific):

| Interface | Endpoint | Transfer type | Maximum packet size |
| --- | --- | --- | --- |
| 0 | `0x02` OUT | Isochronous | 4 |
| 0 | `0x83` IN | Bulk | 512 |
| 0 | `0x05` OUT | Interrupt | 964 |
| 1 | `0x86` IN | Interrupt | 1024 |

This matches the harness's supported interrupt layout.

## Hardware validation

The connected unit completed the shared engine's handshake, reported a current
rate of 44,100 Hz, and returned firmware query bytes `31 01 00`. It uses
interrupt transfers for both PCM directions; the separate bulk endpoint is
MIDI IN. Silent duplex runs completed with 80 audio frames per transfer:

- Three seconds: 132,160 capture frames; full-sized audio transfers, no streaming
  errors. An early cleanup implementation reported an interface-reset error.
- Ten seconds: 440,800 capture frames, approximately 44.1 kHz delivery;
  no streaming errors. This reproduced the cleanup quirk below.
- Repeat three seconds after the cleanup correction: 131,440 capture frames,
  1,644 OUT and 1,643 IN transfer completions, exit status 0, both interfaces
  verified at alternate setting 0 after release.

These are transport tests with silence sent to the playback channels. The
stereo return contained low-level nonzero data; its signal source and decoding
accuracy need a known-signal test. Counts reflect completed frames within a
wall-clock bound, not an exact-duration or latency-compensated recording.
Restarting across these separate runs worked; long-duration stability,
additional rates and behavior under system load are untested.

Two device behaviors required backend handling:

- `GET_INTERFACE` stalls (`LIBUSB_ERROR_PIPE`). The backend reads the current
  alternate setting from Linux sysfs so it need not assume an initial value.
- Setting interface 1 back to alternate 0 can report `LIBUSB_ERROR_OTHER`;
  releasing the interface then completes the reset. Cleanup verifies the final
  setting through sysfs and reports failure if it differs from the original.

The firmware query is variable-length: this A16 returns three bytes even when
15 were requested. The backend accepts the mandatory three-byte prefix;
fixed-size rate/status replies still require their full length.

A subsequent four-second test sent one second of 440 Hz on playback channel 1
at -60 dBFS peak, surrounded by silence, with 10 ms fades. The stereo return
contained 440 Hz during the expected interval, approximately -99.9 dBFS peak
on channel 1 and -110.2 dBFS on channel 2. Other intervals returned to silence.
The run captured 176,080 frames, reported no streaming errors and exited 0 with
both interfaces restored. This establishes a playback-to-capture signal path;
it does not identify the routing as binaural processing, explain the level
attenuation, or establish correct rendering across all playback channels.

Further channel probes (all at -60 dBFS peak per active channel) exercised
channels 1–16 sequentially, then channels 1/2, 1–8 and 1–16 simultaneously.
Fifteen channels returned their assigned 300–900 Hz probe frequencies.
Channel 4 did not return its 420 Hz probe, but did return follow-up 60 and
100 Hz probes. Every playback channel therefore produced a measurable return
at a tested frequency. The frequency-dependent response and left/right levels
must be interpreted through the A16's current configuration; no speaker role,
PRIR, bass-management or other rendering configuration was changed or verified.

During the simultaneous probes, measurable tones remained within 0.23 dB of
their individual levels. Those two main runs (19 and 13 seconds) reported no
unexpected USB transfer errors, completed cleanup, and had no recorded IN
callback gaps above 10 ms. A ten-second low-frequency follow-up also completed
but had four callback gaps over 10 ms (maximum 130 ms), with fewer delivered
frames. Host callback timing does not establish the cause or prove lossless
audio; that remains a timing issue to track independently of device routing.

`channel_experiment.py` generates and analyzes the main probes without USB
access. It uses distinct frequencies per channel and central 0.2-second DFT
windows to compare single-channel and simultaneous tone levels:

```sh
python3 tools/libusb-harness/channel_experiment.py generate experiment-directory
./result/bin/ozzy-libusb stream --seconds 19 --play experiment-directory/sequential-input.wav --capture experiment-directory/sequential-return.wav --log experiment-directory/sequential.log
./result/bin/ozzy-libusb stream --seconds 13 --play experiment-directory/simultaneous-input.wav --capture experiment-directory/simultaneous-return.wav --log experiment-directory/simultaneous.log
python3 tools/libusb-harness/channel_experiment.py analyze experiment-directory
```

## Build

From the repository root, using the included Nix dependency lock:

```sh
nix build path:.
./result/bin/ozzy-libusb --help
```

For development:

```sh
nix develop path:.
cmake -S tools/libusb-harness -B build-userspace -DCMAKE_BUILD_TYPE=Debug
cmake --build build-userspace -j4
ctest --test-dir build-userspace --output-on-failure
```

On another Linux distribution, install a C11 compiler, CMake, pkg-config and
libusb development headers, then use the same CMake commands.

## Inspect before streaming

```sh
./result/bin/ozzy-libusb inspect
./result/bin/ozzy-libusb inspect --device 10:4
```

This enumerates matching A16 devices and prints every configuration, interface,
alternate setting and endpoint descriptor. It does not open the device, claim
interfaces, detach a driver, select settings or send the startup handshake.
Libusb may read descriptors during enumeration. Bus/address selection uses
decimal numbers and changes after a reconnect.

If libusb initialization fails, `LIBUSB_DEBUG=4` provides diagnostic details.
The sandbox must allow libusb's udev hotplug monitor as well as USB device
access. This harness does not disable sandbox restrictions or change system
permissions. A functioning libusb initialization with no matching device is
reported as `No matching A16 found.`

## Bounded streaming

Silence and capture for five seconds:

```sh
./result/bin/ozzy-libusb stream --seconds 5 --capture capture.wav --log usb.txt
```

Play a file once while recording:

```sh
./result/bin/ozzy-libusb stream --seconds 10 --play input.wav --capture return.wav --log usb.txt
```

`--seconds` is required and accepts 1–60. The time limit begins after the
handshake; synchronous control requests have separate two-second timeouts.
Playback files must contain interleaved, packed 24-bit integer PCM, 16 channels
at 44,100 Hz, and fit within the selected duration. Ordinary PCM WAV and
WAVE_FORMAT_EXTENSIBLE with a 24-bit PCM subtype are accepted. Input is
validated before opening USB. Playback is one-shot and the remainder is
silence; there is no channel remapping, resampling, gain adjustment or looping.
Use an intentionally quiet file for first tests. User mode contains software
faults but cannot prevent device misconfiguration or loud output from an
incorrect protocol.

The capture file is stereo PCM WAV. Its contents and routing come from the
device; a binaural return has not been established. Capture is held in bounded
memory and written after transfers stop, so disk I/O cannot block capture
callbacks. A failed run may leave a partial recording; its exit code remains
nonzero. Recorded duration can be shorter than the requested wall time, and
playback/capture have no measured alignment or latency compensation.

`--log` records monotonic nanosecond timestamps, control request fields and
response bytes, and transfer submissions/completions (metadata, not audio
payloads). It can affect scheduling during a test. Capture and log paths must
not already exist. Without a log file, control requests still go to stderr.

If a kernel driver owns an interface, streaming refuses it unless `--detach`
is supplied. That option temporarily detaches the driver. On a clean exit the
backend restores alternate settings, releases claimed interfaces and attempts
to reattach drivers it detached. USB register/rate state is not restored.
Abrupt termination, including SIGKILL, cannot run this cleanup. No persistent
udev rules or driver binding changes are installed.

## Implementation and limitations

The backend runs the engine and libusb completions on one thread. It handles
the A16 profile's bulk/interrupt endpoints and rejects missing or unknown PCM
packet layouts. It does not reset the device or change its configuration.
Sample-rate support stays at upstream's conservative 44.1 kHz setting.
Isochronous-family devices and OS audio-server integration are out of scope.

USB errors, failed submissions and short PCM transfers stop the run rather
than silently padding missing audio. Cancellation drains callbacks before
freeing transfers or their buffers. If that cannot complete within five
seconds, the process terminates without freeing live buffers. SIGINT/SIGTERM
request orderly shutdown once startup finishes. The engine can queue initial
silence before consuming playback samples. Reported playback progress is the
engine's encoded-buffer position, not a measured DAC position.

This reuses Ozzy's handshake, sideband and pacing behavior; it does not resolve
the outstanding A16 protocol questions. Hardware validation should establish
endpoint layouts, channel mapping, capture routing, stable clocking and
restart behavior. No encoded Dolby/DTS mode is exposed.

## Offline verification

CTest covers an A16 fake-bus duplex run, 384 independent single-bit encoder
vectors (16 channels × 24 bits), stereo capture decoding, WAV validation,
CLI rejection of unbounded runs, and the actual libusb backend's cancellation,
disconnect, submission failure, short control response and short PCM paths
through mocked USB I/O. Tests perform no hardware transfers.

```sh
nix develop path:. -c sh -c 'cmake -S tools/libusb-harness -B build-userspace-asan -DCMAKE_BUILD_TYPE=Debug -DHARNESS_SANITIZERS=ON && cmake --build build-userspace-asan -j4 && ctest --test-dir build-userspace-asan --output-on-failure'
```

If the shell preloads the sandbox's udev bridge, GCC's sanitizer runtime must
precede it. Keep the bridge loaded and prepend libasan for the test command:

```sh
nix develop path:. -c sh -c 'LD_PRELOAD="$(cc -print-file-name=libasan.so)${LD_PRELOAD:+:$LD_PRELOAD}" ctest --test-dir build-userspace-asan --output-on-failure'
```
