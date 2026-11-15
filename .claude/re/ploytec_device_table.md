# Ploytec Chipset: Device Table, Codecs and Init Flow

What the official Ploytec macOS driver (`PGDevice` / `PGKernelDeviceXONEDB4`) does for the
Realiser A16, extracted from Ghidra decompilation. The driver uses `this+0xa86` (USB vendor ID)
and `this+0xa88` (USB product ID) for device-specific branching.

## Allen & Heath / Ploytec (VID 0x0A4A)

| USB PID | Device Name | PCM Format | Notes |
|---------|-------------|------------|-------|
| 0x5A16  | Smyth Research Realiser A16 | 24-bit Ploytec, **2 in / 16 out** (`dmaEncode16`) | Excluded from the first `updateAjInputSelector` call in `configurationDone`; the second still runs |

**No serial numbers.** Ploytec firmware has no per-unit serial: the DB2 and DB4 set iSerialNumber = 3 and string 3 is literally `no serial number` (`captures/db2_soulcheck_windows`, `captures/db2_wedigjames`, `captures/db4_linux_18d3e0a`). Two units of the same model can only be told apart by USB port.

## PCM Codec Selection (from configureDevice + chooseISOOutEncoder)

### Device data block layout (at `this+0x3028` pointer)

```
+0x1430: input channel count (iInCh)
+0x1434: input resolution in bits (iInRes)
+0x1438: output channel count (iOutCh)
+0x143c: output resolution in bits (iOutRes)
+0x1440: configuration descriptor index
+0x1444: AjExtData (PtAjExtData)
+0x1464: US-144 config field 1
+0x1468: US-144 config field 2
```

Log: `"configureDevice iInRes=%d, iOutRes=%d, iInCh=%d, iOutCh=%d"`

### Format ID calculation

`format_id = (resolution - 16) / 4` — so 16→0, 20→1, 24→2, 32→4.
Input and output can have different resolutions.

### Output encoders (host → USB)

| Format ID | Bit depth | Encoder functions |
|-----------|-----------|-------------------|
| 0 (16-bit) | 16 | `pcmTo16LSB`, `pcmTo16LSBChannelSwap` (0x133E/0x6000 2ch) |
| 1 (20-bit) | 20 | `pcmTo20LSB` |
| 2 (24-bit) | 24 | `pcmTo24LSB` (default), `pcmTo24Junction4CH`..`10CH` (Ploytec bulk), `pcmTo24TUSBMSB` (0x763), `pcmTo24TUSB2ch` (0xDBA), `pcmTo24Access` (0xA4E), `pcmTo24LSBChannelSwap` (0x133E/0x6000 2ch), `pcmTo24LSB_US144_*` variants |
| 4 (32-bit) | 32 | `pcmTo32LSB`, `pcmTo32Msb` (Elektron Overbridge2) |

### Input decoders (USB → host)

| Format ID | Bit depth | Decoder functions |
|-----------|-----------|-------------------|
| 0 (16-bit) | 16 | `pcmFrom16LSB`, `pcmFrom16LSBChannelSwap` (0x133E/0x6000 2ch), `pcmFrom16LSBChannelSwapMI_4` (0xA4E/0x4040 4ch) |
| 1 (20-bit) | 20 | `pcmFrom20LSB` |
| 2 (24-bit) | 24 | `pcmFrom24LSB` (default), **NULL for Ploytec bulk** (handled separately), `pcmFrom24TUSBMSB` (0x763), `pcmFrom24TUSB2ch` (0xDBA), `pcmFrom24Access` (0xA4E/0x133E 1ch), `pcmFrom24LSBChannelSwap` (0x133E/0x6000 2ch), `pcmFrom24LSBChannelSwapMI_4` (0xA4E/0x4040 4ch) |
| 4 (32-bit) | 32 | `pcmFrom32LSB`, `pcmFrom32Msb` (Elektron Overbridge2) |

Note: Ploytec bulk devices return NULL for the input decoder in `configureDevice` —
input decoding is handled through a separate path (likely the junction decoder).

### Ploytec junction DMA codecs (`chooseDMADecoder` / `chooseDMAEncoder`)
This is the "separate path" for Ploytec bulk/interrupt devices, and the frame format Ozzy implements.
From the XONE_DB2 3.4.9 kext (Ghidra, `DB2_decompile/XONE_DB2_kext_all.c`); the WIZARD_4 3.3.11 kext
has the same function set.

**Input** (`this+0x55b8`), picked by the input channel count (settings `+4`):
- 2..16 channels → `dmaDecode16To02..16At32Bit`, requires `this+0x55b0 == 0x10` (16 lanes)
- 18..32 channels → `dmaDecode32To18..32At32Bit`, requires `this+0x55b0 == 0x20` (32 lanes, frame not decoded yet)
- PID 0xD064 (Dotec) has its own `_DOTEC` variants

The 16-lane input frame is **64 bytes** for any channel count up to 16 (8 frames per call, returns
0x40 bytes per 8 ch / 0x80 per 16 ch of output):
```
bytes 0x00..0x17  bit b (0..7) = sample bit (23 - byte) of channel 2b+1   (odd channels)
bytes 0x20..0x37  bit b (0..7) = sample bit (23 - byte) of channel 2b+2   (even channels)
bytes 0x18..0x1F, 0x38..0x3F unused
```
`dmaDecode16To08` uses bits 0..3 (= Ozzy's 8-channel decoder), `dmaDecode16To16` uses bits 0..7.

**Output** (`this+0x55c0`), picked by the output channel count (settings `+0xc`): `dmaEncode02`,
`04`, `06` (`dmaEncode06_Int964` when the OUT endpoint has wMaxPacketSize 0x3C4 = 964), `08`, …
All write the same **48-byte frame**, 10 frames per 480-byte block:
```
bytes 0x00..0x17  bit b (0..3) = sample bit (31 - byte) of channel 2b+1
bytes 0x18..0x2F  bit b (0..3) = sample bit (31 - byte) of channel 2b+2
```
`dmaEncode08` uses bits 0..3 (= Ozzy's encoder), `dmaEncode02` only bit 0 (channels 1/2), so a
2-channel device is the 8-channel frame with channels 3..8 silent.

Consequence: input lanes (up to 16) and output lanes (up to 8) share one frame format; a device
differs only in how many lanes carry audio. The MixWizard WZ4 (16 in / 2 out) uses
`dmaDecode16To16` + `dmaEncode02`.

## Transfer Types (from configureDevice)

The driver selects between isoc, bulk, and interrupt transfers per device:

| Device type | Input | Output | Flag |
|-------------|-------|--------|------|
| Ploytec bulk (`isPloytecBulkDevice`) | Bulk (fallback: interrupt) | Bulk (fallback: interrupt) | `this[0xb53]` for interrupt |
| JCT30 compatible | Interrupt | Interrupt | `this[0xb53]` = 1 |
| Elektron Overbridge2 | Interrupt | Interrupt | `this[0xb53]` = 1 |
| `isPloytecBulkInIsocOutDevice` | Bulk/interrupt + isoc feedback | Isoc | Mixed |
| Default | Isoc | Isoc | — |

Ploytec bulk devices also have a "keep-alive" isoc out pipe (`m_pcOutPipeIsocKeepAlive`)
on endpoint range 1–4.

### Pipe name → offset mapping
```
m_pcInPipeIsoc            → this+0xab0
m_pcInPipeFeedback        → this+0xab8
m_pcOutPipeIsoc           → this+0xac0
m_pcOutPipeIsocKeepAlive  → this+0xac8
m_pcBulkInPipe            → this+0xa90
m_pcBulkOutPipe           → this+0xa98
m_pcInterruptInPipe       → this+0xaa0
m_pcInterruptOutPipe      → this+0xaa8
```

For Ploytec bulk: endpoint 0x05 is found via `getPipeByAddress('\x05')`. If there is no
EP 0x05, or its **bInterval** is 4 (`pipe+0x16`; `PGPipeDescriptor::scan` @0x209b2 stores
bInterval there, the transfer type is `bmAttributes & 3` at `+0xe`), it falls back to
`getInterruptOutPipe()`. An earlier version of this file read `+0x16 == 4` as a type check.

---

## Output Mixer (from chooseOutMixer)

Based on `this+0x14` (output channel count):
- 2 → `outMixer2CH`
- 4 → `outMixer4CH`
- 6 → `outMixer6CH`

---

## Init Flow (from reqConfigureDevice → configureDevice → configurationDone)

### reqConfigureDevice (outer wrapper)
1. Check firmware update status (`+0x188c == 2` required)
2. `unconfigureDevice` — tear down previous config
3. `configureDevice` — full configuration (see below)
4. `sendSetPicCommand(device, 0x81)` — **unknown vendor request, investigate in Ghidra**
5. Sleep 10ms
6. `registerMidiInterface`
7. Set `+0x1424` = 1 (configuration complete flag)

### configureDevice (USB setup)
1. Elektron Overbridge2: read channel config via vendor reqs 0x11/0x12
2. `findInterfacesInConfig` — find matching USB interfaces
3. `SetConfiguration` (with retry + reset on failure, up to 5 attempts)
4. For A&H 0x6A02/0xF1A0: `SET_INTERFACE` to interface 2
5. `matchInterface` for control, output, input, HID, MIDI interfaces
6. Find and assign pipes (isoc/bulk/interrupt) based on device type
7. `initPipe` for all assigned pipes + bandwidth check
8. Select PCM converters (input decoder + output encoder) based on resolution + device
9. `configurationDone` (see below)

### configurationDone (device init)
1. `updateAjInputSelector` (first call, conditional)
2. `chooseInputPostprocessor` (or `choose6InputPostprocessor` for 0x644/0x8035)
3. `chooseISOOutEncoder`
4. `updateAjInputSelector` (second call)
5. `chooseOutMixer` (if output channel count > 0)
6. `writeDigitalOutSelector`
7. Initialize isoc/bulk/interrupt data structures
8. `USBMidiDevice::configurationDone`
9. `AJ::configurationDone`
10. `setEsuCpldByte`
11. `setElektronChannelMap` (Elektron only)
12. `usbInitDeviceControls` + `usbApplyDeviceControls`
13. `startStreaming` (if pending)

### Device data block layout (at pointer `this+0x3028`)
```
+0x001C: device name string 1
+0x021C: device name string 2
+0x1424: configuration complete flag (1 = configured)
+0x1428: ? (cleared on reconfig)
+0x142c: frame rate (Hz)
+0x1430: input channel count
+0x1434: input resolution (bits: 16, 20, 24, or 32)
+0x1438: output channel count
+0x143c: output resolution (bits)
+0x1440: configuration descriptor index (set to 0 on reconfig)
+0x1444: AjExtData (PtAjExtData) — see vendor_request_I.md
+0x1464: US-144 config field 1
+0x1468: US-144 config field 2
+0x1470: Elektron channel map
+0x1490: Elektron input channel map (8 x uint32)
+0x14B0: Elektron output channel map (8 x uint32)
+0x1A74: XsDeviceControls (0xF04 bytes) — current device controls/selectors
+0x1A7E: output selector byte (within XsDeviceControls)
+0x2978: XsDeviceControls backup copy (0xF04 bytes)
+0x387C: exclusive client 1 (IOUserClient pointer)
+0x3884: exclusive client 2
+0x388C: exclusive client 3
+0x3894: TimeStampsBufferData
```

### Driver version

`v3.4.9` (encoded as 0x3040900 in dispatch case 0x16, string in case 0x3F)

---

## PGDevice Class Layout (updated)

```
this+0x08:   device-specific data (0x14 bytes, cleared by initDeviceSpecificData)
             ESU/0x0A92: this+0x08 = 0x0101
             TEAC 0x8041/0x8048: this+0x18 = 100ms (post-config sleep)
             TEAC 0x8040/0x8046/0x8041/0x8048: this+0x10 = 2, this+0x14 = 2
this+0x0C:   some capability flag (checked in onServiceStart)
this+0x10:   input channel count override
this+0x14:   output channel count override (2, 4, or 6)
this+0x18:   post-config sleep duration in ms (clamped to 1000)
this+0x1E:   Wolfson codec registers array (0xB0 bytes, dispatch 0x29/0x2A)
this+0xCE:   3 x short — some config values (dispatch 0x30)
this+0xD2:   mixer/channel config byte (index 1 value, bits 3-4 reused for index 0 writes)
this+0x190:  ? (dispatch 0x48, 0x80 byte debug buffer)
this+0x218:  mixer mode (0=bypass, 1=active, 2=active variant)
this+0x21C:  mixer knob value (sent as wValue to vendor request 'T')
this+0x21E:  mixer bitmap value (sent as wIndex to vendor request 'T')
this+0x7A0:  capability flag
this+0x7A1:  setFreqOnOutPipe capability flag
this+0x7A8:  has USB1 mode capability
this+0x7A9:  US-144 flag (selects special PCM encoder variants)
this+0x7AC:  has digital output selector (enables writeDigitalOutSelector)
this+0x7D7:  device present flag
this+0x7E4:  Elektron UID (8 bytes, read via vendor req 0x00)
this+0x7EC:  Elektron device name (32 bytes, read via vendor req 0x01)
this+0x798:  ?
this+0x80C:  config changed flag (set on mixer mode change)
this+0x80D:  streaming active flag
this+0x860:  hog ID
this+0xA80:  PT_DeviceDescriptor (parsed USB device descriptor):
  +0xA80: bcdUSB (2 bytes)
  +0xA82: bDeviceClass (1)
  +0xA83: bDeviceSubClass (1)
  +0xA84: bDeviceProtocol (1)
  +0xA85: bMaxPacketSize0 (1)
  +0xA86: idVendor (2) — USB vendor ID
  +0xA88: idProduct (2) — USB product ID
  +0xA8A: bcdDevice (2) — firmware version
  +0xA8C: iManufacturer (1)
  +0xA8D: iProduct (1)
  +0xA8E: iSerialNumber (1)
  +0xA8F: bNumConfigurations (1)
this+0xB53:  interrupt transfer mode flag
this+0xB54:  pending start streaming flag
this+0x1880: PGKernelDeviceXONEDB4 pointer
this+0x1888: USB High Speed flag (set by setIsUSB2Device, NOT a bulk flag)
this+0x188C: firmware update status (must be 2 for normal operation)
this+0x1890: ? (dispatch 0x17)
this+0x1898: 16-byte string (dispatch 0x18)
this+0x3028: pointer to device data block (see layout above)
this+0x3F48: AJ subsystem
this+0x55D0: audio buffer pointer (0x6400 bytes)
this+0x55D8: ESU CPLD byte
this+0x71A8: TimeStampsBuffer pointer
```
