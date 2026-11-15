# Vendor Request 'I' (0x49) — Hardware Control Registers

Sources: USB traffic capture, Windows driver reverse engineering, macOS driver symbol names.
Cross-driver confirmation: Windows and macOS drivers match exactly (same masks, same read-modify-write patterns).

## General Structure

```
bmRequestType: 0x80 (read) / 0x00 (write)
bRequest:      0x49 ('I')
wIndex:        selects sub-register
```

The A16 uses `wIndex = 0`, the AJ input selector / digital status. Other Ploytec devices have
more indices.

Reads return 1 byte in the data phase. Writes encode the value in wValue.

---

## Index 0 — AJ Input Selector / Digital Status

Used in: `updateAjInputSelector`, `setAJDMAInputChannels`, digital lock detection paths.

### Confirmed bits (from Ghidra RE of official macOS driver)

```
bit 0x01 → USB1 mode (set by setUSB1Mode, stored in AjExtData[2])
bit 0x02 → clock source select (set by setClockSource, stored in AjExtData[1])
           AjExtData[1]==0 → set bit (|0x02), non-zero → clear bit (&0xFD)
bit 0x04 → digital lock (confirmed via (byte & 0x04) != 0; triggers re-read after 20ms sleep)
```

### Behavior

- `0x04` indicates digital signal lock
- `0x02` = clock source selection, not generic "input select": set = automatic (the digital input
  when it has a valid signal, else internal), clear = internal. From Tascam's Control Panel:
  menu [Automatic, Internal] → setClockSource(index) → AjExtData[1] 0/1
- `0x01` = USB1 mode flag, only set on certain devices (those with `this[0x7a8]` set,
  or product 0x644 sub-IDs 0x8048/0x8041)
- Driver masks with `0xFD` and recomputes bit 1 from current clock source setting
- Read-modify-write: reads status, modifies bits 0-1 from software state, writes back

### Other bits

```
0x08  → unknown, device-dependent
0x10  → StreamingArmed (from legacy driver naming). Set on powerup for both DB4 and DB2
0x20  → LegacyActive / MODE5. Set by confirm write-back during handshake
0x40  → unknown, device-dependent
0x80  → NOT USB High Speed (see below). Hardware/chipset capability flag, differs between DB4 and DB2
```

- Frequently masked/merged (`& 0xE7`, `& 0x4C`)
- Device-dependent
- Used to encode routing / timing class / hardware mode
- For product 0x644: bits 3-4 (mask `0x18`) come from `this[0xd2]`, rest preserved with `& 0xE7`

### Observed powerup values

| Device | Status | Bits set | Notes |
|--------|--------|----------|-------|
| DB4 (fw 1.4.1) | 0x92 | 0x80 + 0x10 + 0x02 | After confirm → 0xB2 (adds 0x20) |
| DB2 (from capture) | 0x12 | 0x10 + 0x02 | After confirm → 0x32 (adds 0x20) |

Both devices are USB High Speed, yet only the DB4 has bit 7 (0x80) set. This means
bit 7 is NOT "USB High Speed" as the legacy driver naming suggested — it's something
internal to the Ploytec chipset that differs between DB4 and DB2 hardware. Possibly
related to the effects engine, mixer capability, or a different hardware revision of
the Ploytec chip.

The `isPloytecBulkDevice` / `this[0x1888]` flag (USB 2.0 High Speed) is set based
on the USB bus speed, NOT based on this status register bit. They are independent.

### When updateAjInputSelector is called (from Ghidra)

1. **`configurationDone`** — device init, called twice (before and after `chooseISOOutEncoder`)
2. **`setClockSource`** — clock source change: suspend streaming → update → resume
3. **`setUSB1Mode`** — USB mode switch: suspend streaming → update → resume

### _globDummyAjExtData / PtAjExtData structure (inferred)

```
byte +0x00 [0]  → digital lock frequency (read via GET_CUR after write-back)
byte +0x04 [1]  → clock source (PTE_ClockSource enum, controls bit 1 of status byte)
                   For ESU: 2→STATE_A(0x32), 1→STATE_B(0xB0), other→STATE_C(0xB2)
                   Dispatch: read=0x0F, write=0x10 (setClockSource)
byte +0x08 [2]  → USB1 mode (PTE_USB1Mode enum, controls bit 0 of status byte)
                   Dispatch: read=0x02/0x19, write=0x1A (setUSB1Mode)
byte +0x0C [3]  → digital out mode
                   Dispatch: read=0x1B, write=0x1C (setDigitalOutMode)
byte +0x10 [4]  → digital output selector value (written to index 2 by writeDigitalOutSelector;
                   Tascam's panel sets it only from its USB 1.1 mode code; ESU1808: S/PDIF mirror
                   pair 0..3 for chooseDMAEncoder, no request)
                   Dispatch: write=0x03 (setDigitalOutSelector)
byte +0x14      → input postprocessor setting [sub=0]
byte +0x18      → input postprocessor setting [sub=1]
byte +0x1C      → input postprocessor setting [sub=2]
byte +0x20      → output postprocessor setting [sub=0]
byte +0x24      → output postprocessor setting [sub=1]
byte +0x28 [10] → digital lock flag (written from bit 2 of status byte)
```

## Generic selector and confirm (EIE kext 3.3.17, all junction devices)

- `updateAjInputSelector` (configurationDone; not for the ESU devices, which use `_Esu186`, nor
  the Jockey 3 ME): 'I' read (again after 20 ms while bit 2 is set), new = `b | 0x02` (AjExtData[1]
  = 0, the default), bit 0 = AjExtData[2] (default 0) when plist flag 0x7a8 is set; written
  (sign-extended) only if it changed, then 40 ms and GET_CUR. Xones read bit 1 set, so no write.
- `setAJDMAInputChannels` (bulkAudioRun, every stream start, every device with a bulk pipe):
  'I' read, write `b | 0x20 | 0x10` for up to 16 inputs (`b & ~0x20 | 0x10` above; D064 keeps 0x20).
  This is Ozzy's "confirm"; it used to set only 0x20, which is the same on the Xones (bit 4 is
  set at power-up) but not on devices that power up with bit 4 clear.
## Cross-Driver Confirmation

Windows and macOS drivers match exactly:
- Same read-modify-write pattern
- Same bit masks (`0xDF`, `0xE7`, etc.)
- Same logic for digital lock detection and input selector updates

### macOS Function Name → Meaning Map

| Function                 | Meaning                |
|--------------------------|------------------------|
| updateAjInputSelector    | index 0 semantics      |
| setAJDMAInputChannels    | index 0 mode switching |

---

## Enum Definitions

```c
typedef enum VendorRequest : uint8_t {
    VENDOR_REQ_I = 0x49,
} VendorRequest;

typedef enum AjInputSelectorFlags : uint8_t {
    AJ_INPUTSEL_MODE0_BIT        = 0x01,
    AJ_INPUTSEL_INPUT_SELECT_BIT = 0x02,
    AJ_INPUTSEL_DIGITAL_LOCK_BIT = 0x04,
    AJ_INPUTSEL_MODE3_BIT        = 0x08,
    AJ_INPUTSEL_MODE4_BIT        = 0x10,
    AJ_INPUTSEL_MODE5_BIT        = 0x20,
    AJ_INPUTSEL_MODE6_BIT        = 0x40,
    AJ_INPUTSEL_MODE7_BIT        = 0x80,
} AjInputSelectorFlags;
```

### Supporting Constants

```c
#define AJ_INPUTSEL_DIGITAL_LOCK_MASK  0x05
#define AJ_INPUTSEL_DIGITAL_LOCK_VALUE 0x04
```

---

## wValue High Byte Mystery — SOLVED (sign extension artifact)

From Ghidra decompilation of the official macOS Ploytec driver (`PGDevice::updateAjInputSelector`),
the write-back uses:

```c
(short)(char)bVar12
```

This sign-extends the byte through `char` (signed) to 16-bit `short`:
- If bit 7 is set (e.g. `0xB2`): `(char)0xB2` = -78 → `(short)(-78)` = `0xFFB2`
- If bit 7 is clear (e.g. `0x32`): `(char)0x32` = +50 → `(short)(50)` = `0x0032`

**The high byte is not a second register.** It is purely a sign-extension artifact in the
original Ploytec driver. The device likely ignores it.

### Evidence

| Device | Index 0 read | After OR 0x20 | Bit 7 | wValue (sign-extended) |
|--------|-------------|---------------|-------|------------------------|
| DB4    | 0x92        | 0xB2          | set   | 0xFFB2                 |
| DB2    | 0x12        | 0x32          | clear | 0x0032                 |

USB capture from DB2 confirms the official driver sends `wValue = 0x0032`.

### Bug in our driver

`ploytec.c:ploytec_confirm_status()` hardcodes `wvalue = 0xFF00 | modified`, which
always forces the high byte to 0xFF. This is wrong for devices where bit 7 is clear
(e.g. DB2 → sends 0xFF32 instead of 0x0032).

Fix: `wvalue = (uint16_t)(int16_t)(int8_t)modified;`

---

## updateAjInputSelector — Ghidra decompilation analysis

From official macOS driver class `PGDevice` (called on `PGKernelDeviceXONEDB4`).

### deviceRequestStd parameter order (inferred)

```
deviceRequestStd(device, direction, bmRequestType_base, ?, bRequest, wValue, wIndex, data_ptr, length_ptr, timeout)
```

- direction 0x80 = IN (read), 0x00 = OUT (write)
- bmRequestType_base 0x40 = vendor, 0x20 = class
- bRequest 0x49 = 'I', 0x81 = GET_CUR

### Observed behavior

1. Reads index 0: `deviceRequestStd(dev, 0x80, 0x40, 0, 0x49, 0, 0, &byte, &len, 2)`
2. If digital lock bit set (`byte & 0x04`), sleeps 20ms and re-reads
3. Device-specific branching on product IDs at offsets `this+0xa86` and `this+0xa88`:
   - `0x644` = one product family (checks sub-IDs: 0x8041, 0x8040, 0x8048, 0x8046)
   - `0xa4a` = another product family (checks sub-ID: 0xFFDD)
4. Read-modify-write with masks `0x18` and `0xE7` (same as index 1 write constraints)
5. Modifies bit 1 (0x02) and bit 0 (0x01) based on external data (`_globDummyAjExtData`)
6. Write-back uses `(short)(char)bVar12` — sign-extending the byte
7. After write, re-reads index 0 and also reads sample rate via class request (0x81, GET_CUR)
8. Handles digital lock frequency detection with retries (up to ~21 iterations)
9. References `can192()` — checks if device supports 192kHz (0x2ee00 Hz)

## Key Takeaways

- **Not a simple flag register** — it is a stateful control byte
- Index 0 = input + digital lock + mode encoding
- Index 1 = mixer + CPLD + physical switch state
- Many bits must be treated as device-specific composite state, not standalone flags
- Read-modify-write is mandatory; never blindly overwrite
- wValue high byte on writes is a sign-extension artifact, not meaningful data
