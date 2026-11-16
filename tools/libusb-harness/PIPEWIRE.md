# A16 userspace PipeWire driver

Build from the workspace root:

```sh
nix build path:./ozzy-userspace --out-link ozzy-userspace/result
./ozzy-userspace/result/bin/a16-driver
```

The default is **7.1.4t Compatibility**, exposing 12 playback channels.
The card `a16` now advertises **86 profile entries**: 28 Compatibility,
54 Advanced and four PCM Custom bank entries. All four Custom banks have proposed full 16-channel mappings.

Select the profile in KDE's card-profile control, or:

```sh
pactl set-card-profile a16 7.1.4
pactl set-card-profile a16 9.1.6
pactl set-card-profile a16 custom-1
```

Run `a16-driver --list-profiles` to inspect every required room and slot map.
`--profile NAME` selects a startup layout; aliases such as `7.1.4t` are accepted.
WirePlumber may restore a previously saved profile.

- **Compatibility:** a built-in PCM room (or a supported subset of one), with
  every position representable by Pulse.
- **Advanced:** a built-in PCM room/usable subset with native positions Pulse
  cannot name. KDE will show some as `aux`.
- **Custom:** a binding for one of the A16's four PCM Custom banks. These recipes
  use only Pulse-representable positions. The bank and its captures must be
  configured manually to match; no device settings are changed automatically.

The catalog is generated from the supplied Smyth workbook's PCM, Atmos, DTS and
Auro tables. Source-family names describe speaker geometry, not decoding support
for those bitstreams over USB. Only layouts that fit the 16-channel transport
are offered. Inconsistent source rows and omitted custom recipes are recorded
in `layouts/catalog.json`'s audit. See [layouts/README.md](layouts/README.md).

Custom #1 uses all 16 slots: an eight-speaker floor layer including rear center,
LFE, three-across front/rear height rows, and a straight-overhead capture.
Custom #2 also uses all 16: a ten-speaker floor layer including inner fronts
and rear center, LFE, four heights, and straight overhead. These are explicit
custom arrangements. Custom #3 combines inner fronts, front upper center and
true overhead, omitting floor rear center and rear upper center. Custom #4
replaces true overhead with floor rear center. All four base recipes use 16 channels.
The A16's `T` capture maps to Pulse `top-center` / native `TC` (straight overhead).
Edit `layouts/custom-banks.json`, regenerate and rebuild to change the bindings.

A user's capture set can omit speakers without changing these shared recipes.
The bridge reads `$XDG_CONFIG_HOME/open-realiser-a16-driver/config.json` (normally
`~/.config/open-realiser-a16-driver/config.json`) at startup:

```json
{"missing_speakers": ["Chr"]}
```

Use `--config FILE` to select another file, or `--missing-speakers Chr,...` to
override its list (`--missing-speakers ""` clears it). Restart after editing.
Missing identities are removed from every advertised profile, while remaining
speakers retain their original USB slots; omitted USB slots carry silence.
The description shows the effective count, and `a16.missing-speakers` records the
list. `a16.room.required` still describes the complete base room assignments.
Unknown speaker identities or malformed configuration cause startup to fail.

Changing a profile recreates only the playback sink, which can briefly interrupt
playback while clients relink. The stereo capture source and USB engine remain;
the capture source follows the sink's graph, so recording pauses until the
session manager has configured the new sink. Native and Pulse clients see the
same selected profile.

**The user must align the A16 PCM room to the profile's declared assignments.**
A single elevated front/rear row can use Pulse's top-front/top-rear names with
height captures; profiles distinguish the A16's `h` and `t` identities. Layouts
with separate height and top rows may need richer native names or AUX slots.
Nominal position names do not replace measured PRIR angles.

Node properties expose `a16.profile`, `a16.speakers`, `a16.room.required`,
`a16.profile.category`, `a16.profile.advanced` and `a16.profile.pulse-compatible`.

The bridge runs until SIGINT/SIGTERM. For a bounded session, add `--seconds 60`.
`--detach` explicitly permits temporarily detaching an existing kernel driver.
It requires access to the A16 USB device and the user's local PipeWire socket.
Exactly one high-speed A16 must be connected when the bridge starts.
No root privileges are normally required with suitable USB permissions.

The bridge publishes both nodes and opens the A16 at startup, but runs the
handshake and starts streaming over USB only when the playback or return node
first becomes active. From then on it streams until it exits, sending silence
while no client plays, so a later client starts without a USB restart. The first
start takes about 0.6 s for the handshake; audio the first client sends in that
time overflows the playback queue and is dropped.

After a USB failure, including the A16 dropping off USB during its boot
([boot-usb-drop.md](../../docs/development/boot-usb-drop.md)), the bridge closes
the A16, waits for it to come back, resets its USB port (up to three attempts per
reopen) and reopens it, resuming the stream if it had started. The nodes
stay published throughout. libusb hotplug events report the A16 leaving the bus
while no stream is running. The return counts as carrying audio once three
consecutive 100 ms windows each reach an RMS of at least -55 dBFS; the sparse
spikes of a silent return stay well below that. A failure counts as a boot-time
drop while a client is active, before the return has carried audio, and either
within 50 s of the bridge first opening the A16 or within 5 s of the stream
resuming after an earlier boot-time drop. The bridge then waits up to 3 s for the A16 to
return. After any USB failure, once it has reset and reopened the A16, it sends
it nothing until 9 s after the failure, or until the reset finishes if that is
later. A control request that times
out fails the stream at once, so the bridge resets the A16 again without
retrying. In every other case, an A16 off the bus
makes the bridge exit at once with status 0, which removes the nodes. More than
five failures within two minutes end it with status 1. A PipeWire disconnection
ends the bridge.

Run as a systemd service with `Type=notify`, the bridge sends `READY=1` once its
nodes exist and the A16 is open, so a service ordered `After=` it, such as the
upmixer, starts only then. A bridge that exits earlier, as when no A16 is
attached, sends it just before exiting, so systemd records a start followed by
that exit status.

It exposes two selectable desktop nodes:

- `a16.playback`: Realiser A16 USB <selected profile>, a layout-dependent number of playback channels.
- `a16.capture`: Realiser A16 USB Return, 2 recording channels.

The bridge does not issue commands to set default audio routing or the A16's
volume/input/room. The session manager can select newly available devices under
its own policy, especially if no other recording source exists. Select the A16
in an application's audio settings or target the node by name. Do not connect
the capture source back to the playback sink: that creates acoustic-processing
feedback. The sink's software volume is a separate PipeWire control; hardware
headphone volume also affects the USB return.

The USB transport runs at 48000 Hz, 24-bit PCM; PipeWire negotiates interleaved
float audio at 48000 Hz with the bridge and can convert application formats.
The standalone WAV harness still supports all four validated rates; this first
live bridge intentionally fixes its transport to 48 kHz.

## Channel maps

The full 9.1.6t mapping was read from the A16's active Omega room. This program does
not query or change rooms automatically; update the room or adapter map if the
Realiser is configured differently.

| USB | A16 speaker | PipeWire channel |
| --- | --- | --- |
| 1, 2 | L, R | FL, FR |
| 3, 4 | C, SW | FC, LFE |
| 5, 6 | Lss, Rss | SL, SR |
| 7, 8 | Lb, Rb | RL, RR |
| 9, 10 | Lw, Rw | FLW, FRW |
| 11, 12 | Ltf, Rtf | TFL, TFR |
| 13, 14 | Ltm, Rtm | TSL, TSR |
| 15, 16 | Ltr, Rtr | TRL, TRR |

In compatibility mode the 12 graph channels map to USB **1-8, 11-12, 15-16**.
USB **9-10 and 13-14 are explicitly zero-filled**. This allows the current
9.1.6t room to serve both modes without an A16 room change.

The former `auro-13.1` custom recipe is not advertised: its side-height positions
are not Pulse-representable, and the custom-bank policy now excludes those.
Auro-derived layouts matching built-in PCM rooms can still appear in the catalog.
No Auro-3D decoding or Auro-Matic upmixing is added by these profiles.

Example playback with an explicitly mapped 16-channel WAV (select `9.1.6` first):

```sh
pw-cat --playback --target a16.playback \
  --channel-map FL,FR,FC,LFE,SL,SR,RL,RR,FLW,FRW,TFL,TFR,TSL,TSR,TRL,TRR \
  input.wav
```

Record the processed stereo USB return:

```sh
pw-cat --record --target a16.capture \
  --rate 48000 --channels 2 --format s24 return.wav
```

Use a fresh output filename. A recording target must be the capture node,
not the sink's monitor ports, which represent pre-Realiser playback audio.
PipeWire 1.6.9 `pw-cat` returns status 1 on sample-limited recording despite
writing the requested frames: its exit status is set to success only on drain.
For automated checks, verify output frame count and diagnostics as well as status.

## Timing and lifecycle

The playback node drives the PipeWire graph. It is a driver node
(`node.driver`, `priority.driver` 1500) and the return node is in its node group
(`node.group = a16`), so clients of either node run on the cycles the playback
node starts. A timer on PipeWire's data thread starts each cycle at the graph's
quantum and rate: it fills the graph clock (start time, position, duration, the
playback queue level as delay, `rate_diff` and the next start) and triggers the
graph. Each USB out transfer that completes adds 80 frames the A16 has taken. A
completion is reported late but never early, so the earliest-reported of the
last 64 completions dates them. A delay-locked loop, as in PipeWire's ALSA
driver, steers the cycle rate so the graph keeps pace with the A16, and reports
the measured ratio as `rate_diff`. The loop locks once the A16 clock has run for
0.2 s.

The timer runs uncorrected at the graph's rate, with `rate_diff` 1.0, whenever
there is no A16 clock: before USB streaming starts (the first client's first
cycles run before the A16 streams), while the bridge recovers from a USB
failure, and whenever no out transfer has completed for 0.1 s. Clients keep
running on those cycles, and the loop locks afresh when the A16 streams again.
When a client joins the A16 nodes to another device whose driver priority is
higher, that device drives the graph and the A16 nodes follow it.

All engine/libusb calls remain on the USB event thread. PipeWire RT callbacks
perform bounded copies through atomic single-producer/single-consumer queues;
they do not allocate, log, block, invoke libusb, or resample. Each queue holds at
most 16384 frames. Sinc resampling through libsamplerate runs on the USB thread,
with bounded, smoothed queue-level feedback (maximum correction +/-0.5%) to track
clock drift. Capture and playback have independent controllers. While the graph runs on the
A16 clock, their ratios settle near 1.0. Outgoing float
samples are clipped to 24-bit range, with nonfinite values replaced by silence.

The initial queue target is 2048 frames (42.7 ms) per direction, in addition to
PipeWire buffering, USB transfers and the A16's processing latency. Each graph
cycle adds a whole quantum to the playback queue while USB drains it 80 frames
at a time, so the level swings about half a quantum around the target. The
nodes request a 1024-frame quantum (`node.latency`) and allow at most 2048
(`node.max-latency`), which keeps the lowest level about 1024 frames above
empty. The upmixer requests no latency, so it runs at whatever quantum the
graph has. This is a
conservative first implementation, not a low-latency tuned driver. No latency
measurement or compensation is advertised to PipeWire beyond the playback
queue level, which the graph clock carries as its delay. Lossless bit
identity is not promised: adaptive resampling may change samples.

No playback data means silence. An underrun reprimes the playback queue instead
of repeating old samples; source underflow emits silence and reprimes. Full
queues drop incoming frames and increment overrun counts. Capture is discarded
when inactive, with queued old frames flushed on the next source activation.
Stats every five seconds report frames, queue levels, underruns/overruns and
resampling ratios while streaming, then which clock the graph's last cycle
followed (`graph=a16` or `graph=timer`) and its `rate_diff`. SIGINT/SIGTERM or stream errors stop both
sides, cancel transfers and release the interfaces, using the same backend
lifecycle as the WAV harness. The interfaces stay at their streaming alternate
setting, and USB rate/register state is not restored.

## Validation

`nix build` runs the tests of the Cargo workspace, including concurrent queue
ordering across 32-bit counter wrap, capacity limits, PCM conversion/clipping,
simulated +/-300 ppm clock drift for ten minutes, the graph clock's loop on a fake
A16 clock (it converges to the A16 rate through 1.5 ms of completion jitter,
holds 1.0 without a clock, and locks again after a gap), the return-level check, the
engine and libusb bus tests, and the USB lifecycle on a fake A16
(`crates/open-realiser-a16-driver/tests/lifecycle.rs`): on-demand start, boot-time
drops and their recovery timing, control timeouts and the exit statuses. Hardware
evidence, gathered with the C implementation the Rust one replaced, is under
workspace `experiments/pipewire-*`:

- 30-second idle bridge run, both nodes published, correct 16-channel map,
  successful shutdown and interface restoration.
- Live `pw-cat` 16-channel sweep with simultaneous 36-second stereo recording;
  no reported queue xruns. Fifteen full-range positions appeared in the return.
  SW at 80 Hz was weak in both this path and an identical direct USB control,
  consistent with the current room's bass behavior rather than a PipeWire fault.
- The user confirmed hearing the PipeWire sweep.
- Two 30-second silent duplex client sessions with stop/restart passed,
  zero reported queue xruns and successful bridge shutdown/interface restoration.

These are short validation runs, not a guarantee of uninterrupted long-term
operation. Profile validation also covers impulse mapping for every channel of all profiles,
zero-filling unused USB positions, profile enumeration through Pulse, live
profile switching, and stable capture-source identity across switches.
Native PipeWire integration is our addition around Ozzy's shared
engine; it has not been merged upstream.
