# The A16's USB drop during boot

The A16 enumerates on USB early in its boot, while it still shows its boot
splash. About 43.5 seconds after a power-up, it leaves the splash for the main
menu. If playback audio is arriving on USB at that moment, the A16 drops off
USB and re-enumerates. This page records what is known about that moment, how
to recover from the drop, and how the official drivers behave around it.

Times below are seconds after the A16 enumerated following a power-up, with
the USB cable already connected. "Startup requests" means the control requests
the Ploytec engine sends before streaming (`ploytec_handshake`): the `'V'`
read, the `'I'` status reads, the sample rate read and set to 48 kHz, and the
`'I'` write of `0xFFB2`.

## What triggers the drop

| Host activity during the boot | Result |
|---|---|
| None | No drop. The A16 stays enumerated with no configuration or alternate-setting change. |
| Startup requests, both interfaces held at alt 1, no transfers | No drop. |
| Startup requests, then reads pending on the MIDI IN endpoint `0x83` | No drop. `0x83` never returns data. |
| Startup requests, then reads pending on the return endpoint `0x86`, no playback | No drop. No return data arrives. |
| Startup requests, then playback on `0x05` | Drop at 43.1 to 44.1 s. |
| Both interfaces at alt 1, then playback on `0x05`, no other requests | Drop at 43.4 s. |
| Playback for 20 s, stopped at 30 s, then idle | No drop. |
| Control requests four times a second from 11.8 s on | No drop, but the A16 stops answering at 43.3 s and stays that way until a USB port reset. |

- Playback arriving at about 43.5 s is sufficient to trigger the drop, and none of the startup requests is needed. The audio content does not matter: transfers carrying only zero samples trigger it too.
- Playback that stops before 43.5 s leaves nothing behind.
- The drop time follows the boot, not the start of playback. Playback started anywhere from 0.3 to 12.4 s after enumeration dropped at 43.1 to 43.6 s.
- The A16 keeps accepting playback transfers at the sample rate until the drop, so a filling buffer is not the cause.
- After the drop the A16 re-enumerates within 0.3 to 0.8 s.
- Playback that resumes soon after the drop can make the A16 drop again. In one
  boot, streaming resumed 1.0 to 1.4 s after each re-enumeration, and the A16
  dropped at 43.6, 46.1, 48.2 and 50.2 s. During those repeated drops the LED
  next to the SD card flashed repeatedly, and the boot took longer than usual.
  Streams started about 8 s or more after the first drop ran without dropping.
- What the A16 does internally when it leaves the boot splash is not known.

## Cable replug and mid-boot connection

A cable replug of an A16 that has finished booting does not drop. A driver
started right after such a replug streams without a drop. A cable connected
partway through a boot reaches the splash-to-menu change less than 43.5 s
after the A16 enumerates, so no timer counted from enumeration can place it.

## No readiness signal

No readable register changes across the boot:

- vendor request `0x49` (`'I'`), which returns the same byte for wIndex 0, 1 and 2
- vendor request `0x56` (`'V'`), which the Ploytec engine reads as the USB chip's firmware version
- the sample rate (class GET_CUR), on wIndex 0, endpoint `0x86` and endpoint `0x05`
- the standard GET_STATUS

Bit `0x80` of the `'I'` byte varies between boots and does not follow the
boot state. After every power-up, and after the boot drop, the rate reads
44.1 kHz. A cable replug keeps the rate last set.

The return stream does not serve as a signal either. During the boot, with
playback running, return transfers arrive at full length with every sample
zero. Muting, an input other than USB, or a room that does not route the
channels would also return silence. A silent return also carries sparse
single-sample spikes, up to -29 dBFS about every 133 ms, so a single
non-zero sample does not mean audio.

## Recovery after the drop

After the drop the A16 does not answer control requests reliably for
several seconds:

| After the drop | Requests | Result |
|---|---|---|
| about 0.9 s | port reset at 0.6 s, then startup requests | answered; streaming then caused another drop |
| about 3.9 s | port reset at 0.6 s, then startup requests | not answered; the A16 stayed that way until another port reset |
| about 6.4 s | rate and status reads, no reset | answered |
| about 8.1 s | port reset at 1.7 s, which blocked 6.6 s while the kernel retried, then startup requests | answered; streaming held |
| 9 s | port reset at 0.7 to 1.0 s, which finished in 0.2 s, then quiet until 9 s, then startup requests | answered; streaming held (two boots) |

So a request that goes unanswered leaves the A16 unanswering until a USB
port reset (`libusb_reset_device`). A port reset never took the A16 off the
bus: in every logged reset it kept its device number. How long a reset
blocks varies: 0.13 to 0.22 s when the A16 answers it at once, and 6.6 s in
one run where the kernel's first attempt timed out.

A USB port reset also recovers an A16 left unanswering by control requests
sent at 43.3 s during the boot.

## The official drivers

The official Windows and macOS drivers do not detect the boot state. Both
start streaming only when an application starts playing, and stop when
playback stops. Both recover when the stream breaks:

- The macOS driver stops streaming, releases the device configuration, waits
  100 ms, configures the device again and restarts streaming.
- The Windows driver retries each control request up to five times. When the
  stream breaks with the device still present, it configures the device again
  and resumes. When the device appears gone but its port is still connected
  and enabled, it resets the port, waits 20 ms and configures again.
- After repeated breaks the Windows driver sends vendor request `0x72` as a
  device reset. The A16 stalls `0x72` and is otherwise unaffected, so this
  step does nothing on the A16.

## How this driver handles it

The PipeWire bridge streams only while a client plays or records, so the drop
happens only if a client is active at about 43.5 s into a power-up. After the
drop, or any other USB failure, the bridge resets the A16's port as soon as the
A16 is back, retries the reset up to three times and reopens the A16. After a
drop during the boot it sends the A16 nothing until 9 s after the drop, or
until the reset finishes if that is later, and then resumes the stream. A
control request that times out makes it reset the A16 again without retrying.
[PIPEWIRE.md](../../tools/libusb-harness/PIPEWIRE.md) describes the limits.
