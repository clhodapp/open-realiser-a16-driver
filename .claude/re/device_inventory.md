# Device inventory: the Realiser A16 in Ploytec's driver

Built 2026-10-16 from research passes over the official drivers. The detailed findings,
download locations and sources are in the investigation folder, in
`OEM_DRIVERS/findings_xone_protocol.md` (devices on the Xone protocol, `isPloytecBulkDevice`).

The official drivers are downloaded and extracted (never run) under
`OEM_DRIVERS/<Vendor>_<Product>/`. "Plist" means the kext's `PtSpecific.kConfigurationData`
(decoded with `tools/ploytec_config_decode.py`, family flags at blob offsets 0x139 = Xone protocol,
0x13A = bulk in / isoc out). "Fallback" means the per-PID table in `PGDevice::findInterfacesInConfig`
(for kexts without a plist block); "spec" means a vendor spec sheet. Claims marked (?) are
unverified.

## Xone protocol (`isPloytecBulkDevice`)

The A16 is on the Xone protocol: bulk/interrupt PCM out in junction frames with MIDI embedded,
bulk/interrupt PCM in.

| VID:PID | Product | In / out | Rates | Source | Linux | Ozzy needs |
|---|---|---|---|---|---|---|
| 0A4A:5A16 | Smyth Research Realiser A16 | 2 / 16 | 44.1k boot rate, others ? | fallback (3.4.13 kext has no PtSpecific) | ? | in Ozzy (experimental, 44.1k only; 16-lane encoder `dmaEncode16` added) |

The row in `common/ozzy_devices.def` (VID, PID, name, family, in/out, rates, status) is what the
engine runs the device from.
