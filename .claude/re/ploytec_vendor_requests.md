# Ploytec Vendor Requests — Overview

All known USB control requests used by the Ploytec chipset, gathered from
USB traffic capture and Ghidra RE of the official macOS driver (`PGDevice`/`PGKernelDeviceXONEDB4`).

## Vendor Requests (bmRequestType 0xC0 read / 0x40 write)

### 'I' (0x49) — Hardware Control Registers

Detailed in [ploytec_vendor_request_I.md](ploytec_vendor_request_I.md).

Read/write hardware control registers selected by wIndex; the A16 uses wIndex 0, the AJ input
selector / digital status / clock source / USB1 mode.

Reads return 1 byte in data phase. Writes encode the value in wValue (no data phase).
Write wValue is sign-extended from byte via `(short)(char)byte` in the official driver.

### 'V' (0x56) — Firmware Version

```
bmRequestType: 0xC0 (read)
bRequest:      0x56 ('V')
wValue:        0x0000
wIndex:        0x0000
wLength:       15 (0x0F)
```

Returns 15-byte firmware version. Known fields:
- byte[0]: chip ID
- byte[2]: version (decimal encoded: v1.{byte/10}.{byte%10})

## USB Audio Class Requests (standard)

### GET_CUR — Read Sample Rate

```
bmRequestType: 0xA2 (class, interface-to-host)
bRequest:      0x81 (GET_CUR)
wValue:        0x0100
wIndex:        0x0000
wLength:       3
```

Returns 3 bytes, little-endian sample rate in Hz.

Also seen with bmRequestType 0x80/0x20 in `updateAjInputSelector` to read digital lock frequency
after status write-back.

### SET_CUR — Set Sample Rate

```
bmRequestType: 0x22 (class, host-to-interface)
bRequest:      0x01 (SET_CUR)
wValue:        0x0100
wIndex:        0x0086 / 0x0005 (endpoint addresses)
wLength:       3
```

Sends 3 bytes, little-endian sample rate. How many times it is sent differs per source:
- DB4 Windows capture: 5× alternating (3× 0x0086, 2× 0x0005)
- DB2 Windows capture (PR #70): 7× alternating 0x86,0x05,0x86,0x05,0x86,0x05,0x86, ~27 ms apart,
  then GET_CUR with wIndex=0x0086 to read it back
- Official macOS driver: 5× (86,05 in startStreaming; 86,05 in resumeStreaming; 86 + GET_CUR 86 +
  50 ms sleep in bulkAudioRun). Each `setFrequency` call is one request; see `ploytec_sample_rates.md` §5
- Ozzy currently sends 2 (commit bbfa66d, based on the incorrect "once per pipe" reading). Issue #72
  reports the DB2 only works with the 7-call sequence, but that was tested together with the framing
  fix, so it's unproven whether the count matters.

Neither driver ever sends GET_MIN/GET_MAX/GET_RES. The device is never asked which rates it
supports. See `ploytec_sample_rates.md`.

## Standard USB Requests

### SET_INTERFACE (0x0B)

Seen in `configurationDone` for product 0x200c/0x1030:

```
bmRequestType: 0x00
bRequest:      0x0B
wValue:        0
wIndex:        interface number
```

Standard USB SET_INTERFACE to select alternate setting.
