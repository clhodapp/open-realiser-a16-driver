# The A16's USB drop during boot

The A16 enumerates on USB early in its boot, while it still shows its boot
splash. About 43.5 seconds after a power-up it reaches a boot step that
affects its USB side. If playback audio is arriving at that moment, the A16
drops off USB and re-enumerates. This page records what is known about that
step, how to recover from it, and how the official drivers behave around it.

Times below are seconds after the A16 enumerated following a power-up, with
the USB cable already connected.

## What triggers the drop

| Host activity during the boot | Result |
|---|---|
| None | No drop. The A16 stays enumerated with no configuration or alternate-setting change. |
| Full handshake, interfaces held at alt 1, no transfers | No drop. |
| Handshake, then reads pending on the MIDI IN endpoint `0x83` | No drop. `0x83` never returns data. |
| Handshake, then reads pending on the return endpoint `0x86`, no playback | No drop. No return data arrives. |
| Handshake, then playback on `0x05` | Drop at 43.1 to 44.1 s. |
| Interfaces at alt 1, then playback on `0x05`, no vendor or rate requests | Drop at 43.4 s. |
| Playback for 20 s, stopped at 30 s, then idle | No drop. |
| Control requests four times a second through the boot step | No drop, but the A16 stops answering at 43.3 s and stays that way until a USB port reset. |

- Playback arriving at the boot step is sufficient to trigger the drop. None of the handshake requests is needed.
- Playback that stops before the boot step leaves nothing behind.
- The drop time follows the boot, not the start of playback. Playback started anywhere from 0.3 to 12.4 s after enumeration dropped at 43.1 to 43.6 s.
- The A16 keeps accepting playback transfers at the sample rate until the drop, so a filling buffer is not the cause.
- After the drop the A16 re-enumerates within 0.3 to 0.8 s.
- Playback that resumes soon after the drop can make the A16 drop again. In one
  boot, streaming resumed 1.0 to 1.4 s after each re-enumeration, and the A16
  dropped at 43.6, 46.1, 48.2 and 50.2 s. A stream started at 51.8 s ran
  without dropping, as did one resumed about 8 s after the first drop in
  another boot. During those repeated drops the LED next to the SD card
  flashed repeatedly, and the boot took longer than usual.
- The boot step coincides with the A16 leaving its boot splash for the main menu. What the A16 does internally at that point is not known.

## Cable replug and mid-boot connection

A cable replug of an A16 that has finished booting has no boot step. A
driver started right after such a replug streams without a drop. A cable
connected partway through a boot meets the boot step less than 43.5 s after
enumerating, so no timer counted from enumeration can place the step.

## No readiness signal

No readable register changes across the boot:

- vendor request `0x49` (`'I'`), which returns the same byte for wIndex 0, 1 and 2
- vendor request `0x56` (`'V'`), the firmware version
- the sample rate (class GET_CUR), on wIndex 0, endpoint `0x86` and endpoint `0x05`
- the standard GET_STATUS

Bit `0x80` of the `'I'` byte varies between boots and does not follow the
boot state. After every power-up, and after the boot drop, the rate reads
44.1 kHz. A cable replug keeps the rate last set.

The return stream does not serve as a signal either. During the boot, with
playback running, return transfers arrive at full length with every sample
zero. Muting, an input other than USB, or a room that does not route the
channels would also return silence.

## Recovery after the drop

After the drop the A16 answers nothing, standard descriptor reads included,
for about 6 to 7 seconds. A driver that runs its handshake within about a
second of the re-enumeration times out. Restarting at that pace keeps the A16
from answering across many restarts.

A USB port reset (`libusb_reset_device`) issued right after the
re-enumeration recovers the A16. The kernel's reset retries until the A16
answers, which took about 7.4 s after the re-enumeration in one timed recovery. The
handshake then succeeds, and streaming resumes with no second drop. The
return stays silent for about 10 more seconds before carrying audio.

A USB port reset also recovers an A16 left unanswering by control requests
during the boot step.

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
happens only if a client is active at the boot step. After the drop, or any
other USB failure, the bridge resets the A16's port as soon as the A16 is back,
retries the reset up to three times and reopens the A16. After a boot-time
drop it streams nothing for 3 s before resuming the stream.
[PIPEWIRE.md](../../tools/libusb-harness/PIPEWIRE.md) describes the limits.
