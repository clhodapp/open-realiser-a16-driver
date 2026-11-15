# The A16's TCP command server

Since firmware 1.90 the A16 takes commands over TCP and answers them with
ASCII replies. The source is the document "Realiser A16 IP (TCP) Command
Summary" by S. Smyth of Smyth Research, 5 May 2020. Details marked *observed*
come from an A16 on firmware 2.17 (Jan 26 2024) and are not in that document.
The protocol is independent of USB. `crates/a16-control` implements it, and its
`a16-ctl` command uses it.

## Transport

- The A16 listens on one TCP port at a fixed IP address, both set under
  Settings > Network on the A16. The port is 512 to 65535. The other network
  settings cannot be changed while the server is enabled.
- The A16 closes a connection after its "TCP timeout" of inactivity, 30 to
  250 s in steps of 30 s.
- The speaker map screen of either preset shows the server state in a strip at
  its bottom left: blank when nothing is connected, grey when connected, red
  (top half) when a reply goes out, green (bottom half) when a command arrives.
- With "TCP slave response" on, the A16 sends the reply of a function whenever
  that function changes, such as from the IR remote. A client therefore receives
  packets it did not ask for.

## Commands

A command is two hex digits followed by CR LF: `37\r\n`. The document writes
the codes in lowercase, except `B1` to `E2`. *Observed:* the A16 accepts
either case (`AF` and `6A` answered as `af` and `6a` do). `a16-ctl` sends
lowercase.

## Replies

A reply is a sequence of ASCII items, each ended by a NUL. An item is
`KEY=value` or a bare token such as `A01`.

*Observed:*

- Every packet ends with a CR after its last NUL: `PWR=ON\0\r`.
- Some packets contain empty items, runs of consecutive NULs: the firmware
  reply has seven after `APMFW` and seven after `HSRFW`, the `a3` reply starts
  with one, and the `80` reply has one after `DEC`.
- Values are not trimmed. Unassigned channels read `ACH17= `, idle level meters
  `L3=0 `, and headphone names may end in a space (`AQMOD=Focal Clear `).
- Each reply came in one packet, 60 to 110 ms after the command.

`a16-ctl` splits on NUL, drops empty items, and ends a packet at a CR right
after a NUL, or after 200 ms without bytes.

## Connect burst

A connection to the A16 was once seen to start with an unsolicited packet:
`ALL=SOLO\0TEST=OFF\0MODE=SVS\0`, the active-speaker list `A00\0A01\0…` and
the visibility list `V00\0…`. *Observed:* no connection on 2026-11-15 received
it. Two of them waited 3.5 s and 10 s without sending anything; the others got
only replies. When the A16 sends the burst is unknown.

## Table 1: IR remote keys

Each code presses one key of the IR remote. The reply is `KEY=<code>` except
where the table says otherwise.

| Key | Code | Reply | | Key | Code | Reply |
|---|---|---|---|---|---|---|
| POWER | `11` | | | C | `1c` | |
| MENU | `43` | | | L | `1d` | |
| BACK | `59` | `PAGE=0`-`999`, 0 is the home page | | R | `46` | |
| ENTER | `04` | | | Lw | `0d` | |
| CANCEL | `50` | | | Rw | `0c` | |
| ADJ+ | `56` | | | Ls | `15` | |
| ADJ- | `5a` | | | Rs | `45` | |
| UP | `47` | | | Lb | `00` | |
| DOWN | `4b` | | | Rb | `1b` | |
| LEFT | `57` | | | LFE | `4c` | |
| RIGHT | `53` | | | OH | `14` | |
| 1 | `5d` | | | ALL | `1a` | `ALL=SOLO` or `ALL=MUTE` |
| 2 | `5f` | | | headphone | `4d` | `MODE=SVS` |
| 3 | `49` | | | speaker | `5e` | `MODE=AV`, `MODE=MIX` or `MODE=SVS` |
| 4 | `16` | | | A vol+ | `30` | `VA=27`-`99`, `TRI=1`-`3` |
| 5 | `17` | | | A vol- | `31` | `VA=27`-`99`, `TRI=1`-`3` |
| 6 | `44` | | | B vol+ | `33` | `VB=27`-`99` |
| 7 | `55` | | | B vol- | `34` | `VB=27`-`99` |
| 8 | `0f` | | | MUTE A | `32` | `MUTEA=MUTE` or `MUTEA=OFF` |
| 9 | `0e` | | | MUTE B | `35` | `MUTEB=MUTE` or `MUTEB=OFF` |
| 0 | `5b` | | | TEST | `08` | `TEST=LOOP` or `TEST=OFF` |
| * | `12` | | | HT | `42` | `HT=ACTIVE` or `HT=OFF` |
| # | `36` | | | PA | `58` | |
| CAL | `03` | | | PB | `54` | |
| SPK | `0b` | | | | | |
| HP | `0a` | | | | | |

The prose of the document gives the ALL reply as `MODE=SOLO`; its table and
the observed packets use `ALL=`.

## Table 2: other functions

| Function | Code | Reply in the document | Observed on 2.17 |
|---|---|---|---|
| Select eARC, HDMI 1-4, USB, LINE, STEREO, COAXIAL, OPTICAL | `20`-`29` | `IN=eARC`, `IN=HDMI-1` … `IN=OPTICAL` | not sent |
| Power up | `2c` | `PWR=ON` | not sent |
| Standby | `2d` | `PWR=STANDBY` | not sent |
| Power status | `2e` | `PWR=ON`, `PWR=STANDBY` or `PWR=BOOT` | `PWR=ON` |
| Speaker assignments of A and B | `37` | `ACH1=L` … `ACH24=`, `BCH1=L` … `BCH16=` | as documented; an unassigned channel is a space |
| Reset all level meters | `38` | `L1=00` … `L24=00`, `AHL=00` … `BTR=00` | not sent |
| Firmware | `64` | `A16FW`, `APMFW`, `HSRFW`, `HTFW`, `A16SN` | as documented, with padding |
| All levels | `6a` | `L1`-`L24`, `AHL` … `BTR`, `00`-`99` | `L1`-`L24`, then `AHL AHR ATL ATR BHL BHR BTL BTR`; values not zero-padded (`92`, `0 `) |
| User A preset 1-16 | `70`-`7f` | `PA=01` … `PA=16` | not sent |
| User A info | `80` | see below | see below |
| User A volume | `83` | `VA=27`-`99`, `TRI=1`-`3` | `VA=50` |
| User A head-tracker angle | `85` | `AHT=string` | `AHTA=0`, `AHTE=0` |
| User B preset 1-16 | `90`-`9f` | `PB=01` … `PB=16` | not sent |
| User B info | `a0` | see below | see below |
| User B volume | `a3` | `VB=27`-`99` | an empty item, then `VB=50` |
| User B head-tracker angle | `a5` | `BHT=string` | `BHTA=0`, `BHTE=0` |
| Speaker key visibility | `ae` | `V00`, `V01` … `Vnn` | the `V` list, the `A` list, then `ALL`, `TEST`, `MODE` |
| Active speaker status | `af` | `A00`, `A01` … `Ann` | `ALL`, `TEST`, `MODE`, then the `A` list |
| Select speaker ID 1-50 | `B1`-`E2` (`0xB0` + ID) | solo mode: `A00 Ann`; mute mode: toggles `Ann` / `Mnn` | not sent |

In the speaker lists, `nn` is a speaker ID from the table below. `A00` and
`V00` start a list; the IDs follow in the A16's order, not by number:
`V00 V01 V02 V03 V04 V17 V18 V09 V10 V07 V08 V11 V25 V26 V29 V30 V24`.

The document does not explain the `A` and `E` suffixes of `AHTA` and `AHTE`.

### User info

`80` answers, in this order on 2.17:

| Key | Meaning |
|---|---|
| `AUR` | user name |
| `PA` | preset number |
| `VA` | volume |
| `TRI` | tri-level volume, only when tri-level is enabled |
| `AROOM` | room description |
| `ANAME` | subject name |
| `ASPKR` | room layout |
| `AQFILE` | HPEQ file name |
| `AQNAME` | HPEQ subject |
| `AQDATE` | HPEQ date, `17:35 20/06/2022` (*observed*, not in the document) |
| `AQTYPE` | HPEQ filter type |
| `AQMOD` | HPEQ headphone model |
| `ATACT` | tactile output, `ON` or `OFF` (*observed:* empty) |
| `IN` | audio source |
| `DEC` | active decoder |
| `LM` | active listening mode |
| `UMIX` | upmixer mode |
| `HTMODE` | head-tracker mode |
| `LEG` | Dolby/DTS legacy mode |
| `USER` | single or dual user mode (*observed:* `DUAL`) |

`a0` answers `BUR`, `PB`, `VB`, `BROOM`, `BNAME`, `BSPKR`, `BQFILE`,
`BQNAME`, `BQDATE`, `BQTYPE`, `BQMOD` and `BTACT`, the same fields for user B.
The fields from `IN` on describe the whole A16 and come only with `80`.

The observed replies are the test fixtures in
`crates/a16-control/src/fixtures.rs`.

## Speaker IDs

From Tables 3 and 4 of the document. IDs 51 and 52 cannot be selected.

| ID | Designator | Class | | ID | Designator | Class |
|---|---|---|---|---|---|---|
| 1 | L | floor | | 27 | Lhs | height |
| 2 | R | floor | | 28 | Rhs | height |
| 3 | C | floor | | 29 | Lhr | height |
| 4 | SW | LFE | | 30 | Rhr | height |
| 5 | Ls | floor | | 31 | Ltf | overhead |
| 6 | Rs | floor | | 32 | Rtf | overhead |
| 7 | Lb | floor | | 33 | Ltm | overhead |
| 8 | Rb | floor | | 34 | Rtm | overhead |
| 9 | Lss | floor | | 35 | Ltr | overhead |
| 10 | Rss | floor | | 36 | Rtr | overhead |
| 11 | Cr | floor | | 37 | Lsc | floor |
| 12 | SW2 | LFE | | 38 | Rsc | floor |
| 13 | Lw | floor | | 39 | Ls1 | floor |
| 14 | Rw | floor | | 40 | Rs1 | floor |
| 15 | Lbs | floor | | 41 | Lrs1 | floor |
| 16 | Rbs | floor | | 42 | Rrs1 | floor |
| 17 | Lc | floor | | 43 | Lrs2 | floor |
| 18 | Rc | floor | | 44 | Rrs2 | floor |
| 19 | Lu | under | | 45 | Lhw | height |
| 20 | Ru | under | | 46 | Rhw | height |
| 21 | Cu | under | | 47 | Lhs1 | height |
| 22 | Ch | height | | 48 | Rhs1 | height |
| 23 | Chr | height | | 49 | Lbu | under |
| 24 | T | overhead | | 50 | Rbu | under |
| 25 | Lh | height | | 51 | SW | LFE |
| 26 | Rh | height | | 52 | SW | LFE |

The classes are the colours of Table 4, named by its legend.

## Unknown

- When the connect burst is sent.
- The replies on 2.17 to the commands that change the A16: keys, inputs,
  power, presets, volume, mute, `38` and speaker selection. Only the read-only
  requests were sent.
- What the A16 does with a code that has no function, and whether it answers
  commands in standby.
- The meaning of `ATL`, `ATR`, `BTL` and `BTR` in the levels, and the units of
  the head-tracker angles.
- Whether a reply can span packets on a slow link.

## a16-ctl

```sh
a16-ctl --host 192.168.1.53 --port 4101 status
A16_COMMAND=realiser-a16-tcp a16-ctl watch
a16-ctl --host 192.168.1.53 --port 4101 send preset a 3
a16-ctl --host 192.168.1.53 --port 4101 raw 6a
```

`status` requests `2e`, `64`, `80`, `a0`, `37` and `af` and prints the state
they describe. `watch` prints every item with a UTC timestamp. `send` takes
named commands (`a16-ctl --help` lists them) and prints the reply; packets that
arrive meanwhile are printed with the prefix `unsolicited:`. `raw` sends any
code. `status` and `send` wait up to 2 s per command for a packet that holds
the reply key of that command, such as `PWR` for `2e`.
