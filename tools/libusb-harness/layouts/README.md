# A16 USB layout catalog

The initial catalog has 86 profile entries, including four Custom bank entries.
All four Custom banks have proposed full 16-channel mappings.
They describe what the user must configure on the A16; the driver does not
load PRIRs or edit device rooms. Catalog entries have not all been auditioned.

Files:

- `smyth-tables.json`: frozen factual rows from the official configuration workbook,
  with workbook SHA256 and source row numbers (266 rows, October 2022 workbook).
- `custom-banks.json`: the four editable PCM Custom bank bindings. Empty slot
  lists mean unassigned/unselectable, not a playable zero-channel layout.
- `generate.py`: generator; `--check` verifies that generated artifacts are current.
- `catalog.json`: complete advertised maps, source aliases, required rooms and audit.
- `../profiles_generated.inc`: generated C data. Builds do not need the workbook.

Regenerate from this directory after editing bank bindings:

```sh
python generate.py
```

The optional `import_workbook.py INPUT.xlsx OUTPUT.json` imports the workbook
using Python's standard library. It does not execute Excel formulas or macros.

## Rules

At most 16 independent channels. Distinct speaker identities, USB slots and
native position IDs are preserved. A source layout with <=16 speakers in slots
above 16 can be repacked only into a declared PCM Custom room. Standard catalog
entries are retained only when their mapping fits a built-in PCM room (possibly
with unused extra captures). Other recipes belong to the four Custom banks.

Compatibility is checked by position vocabulary. Front wides, side tops, a
second LFE, and certain other directions require Advanced native positions.
Front/rear height rows may use the same conventional Pulse top-front/top-rear
names, while their A16 identities and expected captures remain explicit. If
separate height and top layers coexist, native height/AUX codes preserve separate
channels. A native code is not an exact measurement of capture geometry.

Some workbook rows contradict their mode names. Most inconsistent rows are
excluded with an audit entry; the specific PCM 9.1.2m extra-SW2 typo is corrected
explicitly. Other valid families often supply the same missing layout. Entries
with >16 independent channels are excluded. No promise is made that this dated
workbook enumerates every mode added in later firmware.

## Custom bank recipes

These are configurable starting points, not claims of standard named layouts.
Matching Omega captures and PCM room configuration must be verified before use.

### custom-1: PCM Custom #1: 8.1.7h, three-across heights + rear center + overhead

16 channels. PCM Custom #1 with USB 1=L, 2=R, 3=C, 4=SW, 5=Lss, 6=Rss, 7=Lb, 8=Rb, 9=Cr, 10=Lh, 11=Ch, 12=Rh, 13=Lhr, 14=Chr, 15=Rhr, 16=T; other slots unused.

### custom-2: PCM Custom #2: 10.1.5h, inner fronts + rear center + heights + overhead

16 channels. PCM Custom #2 with USB 1=L, 2=R, 3=C, 4=SW, 5=Lc, 6=Rc, 7=Lss, 8=Rss, 9=Lb, 10=Rb, 11=Cr, 12=Lh, 13=Rh, 14=Lhr, 15=Rhr, 16=T; other slots unused.

### custom-3: PCM Custom #3: 9.1.6h, inner fronts + front upper center + true overhead

16 channels. PCM Custom #3 with USB 1=L, 2=R, 3=C, 4=SW, 5=Lc, 6=Rc, 7=Lss, 8=Rss, 9=Lb, 10=Rb, 11=Lh, 12=Ch, 13=Rh, 14=Lhr, 15=T, 16=Rhr; other slots unused.

### custom-4: PCM Custom #4: 10.1.5h, inner fronts + front upper center + rear center (no overhead)

16 channels. PCM Custom #4 with USB 1=L, 2=R, 3=C, 4=SW, 5=Lc, 6=Rc, 7=Lss, 8=Rss, 9=Lb, 10=Rb, 11=Lh, 12=Ch, 13=Rh, 14=Lhr, 15=Rhr, 16=Cr; other slots unused.

## Advertised catalog

| Profile ID | Category | Channels | Matching built-in PCM room |
| --- | --- | --- | --- |
| `7.1.4` | Compatibility | 12 | 7.1.4t |
| `9.1.6` | Advanced | 16 | 9.1.6t |
| `2.0` | Compatibility | 2 | 2.0.0 |
| `2.2.2m` | Advanced | 6 | 2.2.2m |
| `2.2.2h` | Advanced | 6 | 2.2.2h |
| `3.0` | Compatibility | 3 | 3.0.0 |
| `3.2.2m` | Advanced | 7 | 3.2.2m |
| `3.2.2h` | Advanced | 7 | 3.2.2h |
| `4.0` | Compatibility | 4 | 4.0.0 |
| `4.2.2m` | Advanced | 8 | 4.2.2m |
| `4.2.2h` | Advanced | 8 | 4.2.2h |
| `4.2.4t` | Advanced | 10 | 4.2.4t |
| `4.2.4h` | Advanced | 10 | 4.2.4h |
| `5.0` | Compatibility | 5 | 5.0.0 |
| `5.1` | Compatibility | 6 | 5.1.0 |
| `5.2.2m` | Advanced | 9 | 5.2.2m |
| `5.2.2h` | Advanced | 9 | 5.2.2h |
| `5.2.4t` | Advanced | 11 | 5.2.4t |
| `5.2.4h` | Advanced | 11 | 5.2.4h |
| `5.2.6t` | Advanced | 13 | 5.2.6t |
| `6.0` | Compatibility | 6 | 6.0.0 |
| `7.0` | Compatibility | 7 | 7.0.0 |
| `7.1` | Compatibility | 8 | 7.1.0 |
| `7.1.2m` | Advanced | 10 | 7.1.2m |
| `7.2.2m` | Advanced | 11 | 7.2.2m |
| `7.1.2h` | Compatibility | 10 | 7.1.2h |
| `7.2.2h` | Advanced | 11 | 7.2.2h |
| `7.2.4t` | Advanced | 13 | 7.2.4t |
| `7.1.4h` | Compatibility | 12 | 7.1.4h |
| `7.2.4h` | Advanced | 13 | 7.2.4h |
| `7.1.6t` | Advanced | 14 | 7.1.6t |
| `7.2.6t` | Advanced | 15 | 7.2.6t |
| `9.0` | Advanced | 9 | 9.0.0 |
| `9.1.2m` | Advanced | 12 | 9.1.2m |
| `9.2.2m` | Advanced | 13 | 9.2.2m |
| `9.1.2h` | Advanced | 12 | 9.1.2h |
| `9.2.2h` | Advanced | 13 | 9.2.2h |
| `9.1.4t` | Advanced | 14 | 9.1.4t |
| `9.2.4t` | Advanced | 15 | 9.2.4t |
| `auro-2.1` | Compatibility | 3 | 2.2.2m |
| `auro-3.1` | Compatibility | 4 | 5.1.0 |
| `auro-4.1` | Compatibility | 5 | 5.1.0 |
| `auro-222.0` | Compatibility | 6 | 4.2.2h |
| `auro-222.1` | Compatibility | 7 | 4.2.2h |
| `auro-8.0` | Compatibility | 8 | 4.2.4h |
| `auro-8.1` | Compatibility | 9 | 4.2.4h |
| `auro-5.0.2h` | Compatibility | 7 | 5.2.2h |
| `auro-5.1.2h` | Compatibility | 8 | 5.2.2h |
| `auro-7.0.6t` | Advanced | 13 | 7.1.6t |
| `2.2` | Advanced | 4 | 2.2.2m |
| `3.2` | Advanced | 5 | 3.2.2m |
| `5.2` | Advanced | 7 | 5.2.2m |
| `5.0.2m` | Advanced | 7 | 5.2.2m |
| `5.1.2m` | Advanced | 8 | 5.2.2m |
| `5.0.4t` | Compatibility | 9 | 5.2.4t |
| `5.1.4t` | Compatibility | 10 | 5.2.4t |
| `5.0.4h` | Compatibility | 9 | 5.2.4h |
| `5.1.4h` | Compatibility | 10 | 5.2.4h |
| `7.2` | Advanced | 9 | 7.2.2m |
| `7.0.2f` | Compatibility | 9 | 7.1.2h |
| `7.0.2m` | Advanced | 9 | 7.1.2m |
| `7.0.4t` | Compatibility | 11 | 7.1.4t |
| `7.0.4h` | Compatibility | 11 | 7.1.4h |
| `9.0.2f` | Advanced | 11 | 9.1.4t |
| `9.1.2f` | Advanced | 12 | 9.1.4t |
| `9.0.2m` | Advanced | 11 | 9.1.2m |
| `9.0.2h` | Advanced | 11 | 9.1.2h |
| `2.0.2m` | Advanced | 4 | 2.2.2m |
| `2.1.2m` | Advanced | 5 | 2.2.2m |
| `3.0.2m` | Advanced | 5 | 3.2.2m |
| `4.2` | Advanced | 6 | 4.2.2m |
| `4.0.2m` | Advanced | 6 | 4.2.2m |
| `4.1.2m` | Advanced | 7 | 4.2.2m |
| `5.0.6t` | Advanced | 11 | 5.2.6t |
| `5.1.6t` | Advanced | 12 | 5.2.6t |
| `6.1` | Compatibility | 7 | 7.1.0 |
| `6.0.2m` | Advanced | 8 | 7.1.2m |
| `6.1.2m` | Advanced | 9 | 7.1.2m |
| `8.0` | Advanced | 8 | 9.0.0 |
| `9.0.4t` | Advanced | 13 | 9.1.4t |
| `9.0.6t` | Advanced | 15 | 9.1.6t |
| `9.1` | Advanced | 10 | 9.1.2m |
| `custom-1` | Custom | 16 | Custom bank |
| `custom-2` | Custom | 16 | Custom bank |
| `custom-3` | Custom | 16 | Custom bank |
| `custom-4` | Custom | 16 | Custom bank |
