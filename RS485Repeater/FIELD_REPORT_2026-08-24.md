# Field investigation — 2026-08-24

## Outcome

The original repeater could not decode the upstream bus because the installed FT232/repeater
differential pair has the opposite polarity to the MAX3362 logic convention. Repeater v2.1.6
corrects side 1 in the ESP32 UART peripheral, keeps panel-side polarity normal, forwards complete
COBS frames under one DE assertion, and exposes independent USB diagnostics.

After flashing v2.1.6, one addressed position request and one complete 36-byte reply were observed
for every module ID 1 through 9. Final repeater counters were:

- side 1: 81 RX/forwarded bytes, 9 frames, 0 incomplete, 0 active-RX UART errors
- side 2: 324 RX/forwarded bytes, 9 frames, 0 incomplete, 0 active-RX UART errors
- 0 simultaneous starts and 0 ownership errors
- 9 expected side-1 `turnaround_events` while RE# disabled the receiver during reply transmission

## Module status

All modules reported application `Portal v2026-08-20_12.46 1439931` and the provisioned serial
matching their bus address. No module firmware was changed.

| ID | Axis A | Axis B |
|---:|:---:|:---:|
| 1 | fail | fail |
| 2 | pass | fail |
| 3 | fail | fail |
| 4 | fail | fail |
| 5 | fail | fail |
| 6 | pass | pass |
| 7 | pass | fail |
| 8 | fail | fail |
| 9 | pass | fail |

## Representative startup diagnosis

An addressed startup retest was run on module 1 and stopped after the failure class was proven:

- Axis A completed its mechanical cycle at 5,930 full steps (5,928 expected).
- Axis B reported `Switch not seen` during the cycle test.
- Axis A's optical fast-home could not find a flag at thresholds 253 or 254.
- It only acquired at the absolute ceiling, 255. The measured crossing was 253, leaving two PWM
  counts of usable optical margin—the firmware's minimum accepted margin.
- The flag then disappeared at the precision edge speed. Firmware classified the result as
  `speed-dependent-edge` / `operating point: flag not repeatable at edge speed`.

This is not a fleet-wide startup-firmware defect: the same binary passes both axes on module 6,
and the failure distribution is axis-specific. The failed axes lack reliable optical contrast.

## Physical remediation and acceptance test

Inspect the failed axes' reflective home flags and optical paths. Restore the production reflective
silver coating where it is absent/dim, clean the flag and sensor window, and correct sensor/flag
alignment or cover interference. Do not field-flash the nine modules for this symptom.

After physical work, run the `startup` plan for each address. Accept an axis only when all four
health flags (`measure_cycle_ok`, `switches_ok`, `backlash_ok`, `home_ok`) are true. Accept the
panel when all nine modules pass and the repeater retains zero active-RX UART, incomplete-frame,
and ownership errors.

## Recovery hashes

- Original 4 MB ESP32 flash readback:
  `fff905970c6a308a7c725e24a9532b975aa20890d5b7df490ab88221c22f9950`
- Installed v2.1.6 factory image:
  `31438fbaaf2e95a38a54bb13f0bc8f657cac352f9e6c7b8390924ac1658be8d0`

## Reworld V3 routing follow-up

Repeater v2.2.0 was installed on the same ESP32 without changing any Portal
firmware. It replaces the streaming bridge with bounded complete-frame
queues and learns the local contiguous nine-ID block from valid inner
replies.

After the original report, the crossed upstream A/B wiring was corrected
physically. The v2.1.6 side-1 software inversion was therefore removed;
v2.2.0 uses normal polarity on both UARTs.

Before the physical polarity correction, a non-motion acceptance run using
the temporary side-1 inversion reset counters, requested positions from IDs
1 through 9, requested non-local ID 10, and submitted a keyframe covering
only IDs 10 through 18. This verified the routing behavior:

- IDs 1–9 each returned one complete 36-byte position reply from the
  expected source.
- Routing mode became `filtered` with range `1–9`.
- The ID 10 unicast and non-intersecting keyframe were both filtered; the
  inner bus byte count did not increase for either.
- Both queues reached a high-water mark of one frame and drained to zero.
- Active-RX UART errors, incomplete/oversized frames, queue drops, parse
  errors, topology conflicts, and transmission errors all remained zero.
- The nine side-1 turnaround events corresponded to the nine expected
  reply transmissions while that transceiver's receiver was disabled.

After building and flashing the final normal-polarity image, the currently
connected upstream path produced one UART error and zero decoded bytes for a
single position request. This is the electrical signature previously seen
with reversed polarity, so the final normal-polarity image cannot yet pass
the connected-bench acceptance test. Verify the present USB-RS485-to-side-1
A/B connection before repeating the run. The six-repeater/54-Portal
WaveShare topology and 5 ms host pacing also require the production soak
described in `Protocol.md`.

Final artifacts:

- Application image (normal polarity on both sides):
  `e8fd55d32703e21e3719d34a7495bfcf54260d0e0f05a8009c3d481061d919ef`
- Factory image (normal polarity on both sides):
  `14a13fc1a9826672c4cce05a19a704c710d184f3e87235187939f9c3b3ce4d68`

## Post-correction rerun

The external A/B pair was changed and the first Portal was connected to the
programmer board. Repeater v2.2.0 was left at the production setting of normal
polarity on both sides; no Portal firmware was written.

The three serial endpoints were verified against their USB parent identities
and by direct identity queries before interpreting the rerun:

- `/dev/cu.usbserial-AR9366BD`: FTDI FT232R, serial `AR9366BD` — the
  USB-RS485 adapter used for upstream wire frames
- `/dev/cu.usbmodem21203`: STM32 ST-Link V2-1 VCOM — direct Portal serial;
  querying it returned `Portal v2026-08-20_12.46 1439931`
- `/dev/cu.usbmodem21401`: Espressif USB JTAG/serial — the repeater console;
  querying it returned repeater version 2.2.0

After the ST-Link was power-cycled, both of the programmer-board paths passed
their read-only checks:

- The VCOM smoke plan passed in 1.154 s with 40 received packets and no decode
  errors. The module identified as Portal serial 1 running
  `Portal v2026-08-20_12.46 1439931`.
- SWD readback identified an STM32G070RBT6, UID
  `00240052-30355107-35303836`, RDP level 0, a valid split
  bootloader/application layout, provision serial 1, and the same production
  firmware. The application SHA-256 was
  `d21f37f1e4d61b3f277d2a1402f68e744bc596e76d7528499ebaf90b18675f5b`.
- Persisted settings were readable: 150 mA operating current, full-current
  home recovery enabled, axis A threshold/width 235/299, and no valid axis B
  calibration record (default 255/-1).

After the motor/serial tests, the VCOM link remained healthy but the app's SWD
state returned to `ProbeGone`; a software rescan did not recover it. Since the
same session had already completed the UID, option-byte, settings, and full
flash readback, this is a programmer-board/USB stability fault rather than
evidence of corrupt Portal flash. Power-cycling the ST-Link recovered it once;
check the programmer USB/power/SWD harness if it recurs.

The non-destructive `startup` plan then reproduced the module fault over the
direct VCOM path. It failed after 10.727 s when axis A reported
`routineFindSwitchAccurate: Switch not seen`. The module's continuing routine
subsequently reported the same `Switch not seen` failure for axis B before it
was escaped. This direct connection bypasses the repeater and confirms that
the LED failure is produced by the Portal's mechanical/sensor path, not by
RS485 routing or a corrupt application image.

The corrected external RS485 path still did not pass. A burst discovery was
discarded as an acceptance result because it overran the four-frame diagnostic
queue. The test was repeated in isolation: the bench RS485 worker was
disconnected, repeater counters were cleared, and exactly one valid ID-1
position request (`03 93 01 05 81 A1 70 C0 00`) was sent. The host received no
reply. Repeater counters after that single request were:

- side 1: 0 RX bytes, 0 decoded/forwarded frames, 1 active-RX UART error
- side 2: 0 RX bytes and 0 frames
- both queues empty, with no drops, parse failures, or TX errors

An upstream-only confirmation then targeted filtered, non-local ID 10 so no
frame would be sent to the Portal branch. Five valid nine-byte COBS frames
were sent from the USB-RS485 adapter at 200 ms intervals. Side 1 recorded
zero RX bytes, zero decoded frames, and exactly five active-RX UART errors;
side 2 remained completely idle. The one-error-per-transmission result makes
the host-side electrical/UART polarity failure deterministic and independent
of repeater routing, queueing, and Portal firmware.

After returning the external A/B pair to its earlier orientation, both ESP32
UART polarities were compared using the exact same valid host-only frame and
five-transmission pacing. With normal polarity, side 1 again recorded 0 of 45
bytes and five UART errors. With side-1 inversion enabled, it recorded 25 of
45 bytes with zero UART errors, but produced five incomplete frames and zero
decoded frames. Side 2 stayed idle in both cases. The normal production image
was restored after the comparison. Since neither polarity recovers a complete
frame, this is not a simple A/B selection problem; check for an open conductor,
poor termination/bias/reference, insufficient differential swing, or incorrect
USB-RS485 driver-enable behavior.

A subsequent repeat produced the same ESP32 counters for both polarities, but
the FTDI adapter now read back all 45 transmitted bytes in both runs (previous
runs had no adapter RX echo). That change does not represent repeater reception:
side 1 still decoded no complete frame. It does show that the adapter receiver
is now seeing its own bus transmission, making transceiver driver-enable/echo
configuration or the connection between the adapter terminals and MAX3362 a
particularly important next check.

### UART-format audit

The sender/receiver format was audited after FTDI echo was disabled and the
adapter ground was disconnected:

- Repeater side 1 explicitly calls `Serial0.begin(115200, SERIAL_8N1, ...)`.
- RouterRS explicitly opens 115200 baud, eight data bits, no parity, and one
  stop bit. PortalFW and the legacy Router also select 115200 and their UART
  libraries' 8N1 defaults.
- The live pyserial and macOS termios state for FTDI serial `AR9366BD` both
  reported 115200 input/output speed, eight data bits, no parity, one stop bit,
  and no XON/XOFF, RTS/CTS, or DSR/DTR flow control.
- All four static RTS/DTR level combinations produced the identical normal-
  polarity result: zero received bytes and one UART error per transmitted
  frame. Those control lines are therefore not selecting a working adapter
  direction mode.
- Sender baud was swept across 9,600, 19,200, 38,400, 57,600, 76,800,
  115,200, 230,400, 250,000, and 460,800, with both normal and inverted ESP
  polarity. 115,200 was also tested as 8N2, 8E1, and 8O1. No combination
  produced a valid protocol frame. Inverted/38,400 created delimiter-like
  garbage, but all six fragments failed parsing and three streams timed out;
  it was not an alternate working format.
- All 30 `router-proto` COBS, MessagePack envelope, command, reply, and golden-
  frame tests passed. The transmitted host-only frame is consequently valid
  protocol data, not a malformed test vector.

This rules out a conventional baud, data-bit, parity, stop-bit, software flow-
control, RTS/DTR, or COBS/MessagePack mismatch. The remaining fault boundary is
the electrical signal between the FTDI adapter's local RS485 transceiver and
the repeater MAX3362/RO path. The normal-polarity image was restored after the
audit.

A subsequent full-system power cycle did not change that boundary. Before
counters were cleared, the freshly booted repeater had already accumulated
three side-1 UART errors and a three-byte incomplete side-2 stream. After
`reset-counters`, five controlled 115200/8N1 outer frames again produced zero
side-1 RX bytes, zero complete frames, and exactly five active-RX UART errors;
adapter echo and side-2 activity were both zero. The failure therefore survives
a cold restart of the adapter, repeater, transceivers, and Portal panel.

That failure occurs before routing and before any Portal sees the request. It
is the same signature as an inverted, open, or mis-paired host-side
differential connection. Verify polarity electrically at the repeater side-1
MAX3362 pins rather than relying on USB-RS485 A/B labels, and check both
conductors, the common reference, termination/bias, and that the adapter is on
the host-facing side. Keep the normal-polarity v2.2.0 image installed while
checking the wiring.

The final software verification rerun passed all 10 native BridgeCore tests
and the ESP32-C3 release build. This build check was not uploaded; the tested
normal-polarity v2.2.0 field image remains installed.

## Resolution — 2026-08-25

**The fault was the USB-RS485 adapter, not the wiring, the polarity, or any
electrical property of the differential pair.** FTDI FT232R serial `AR9366BD`
does not provide automatic half-duplex direction control: its transceiver
driver-enable follows RTS, so with RTS at any static level it never drives the
bus at all. Every measurement in the sections above was taken through an
adapter that was not transmitting.

This section was produced on a different computer and a different ESP32-C3 from
the one above, with the same `AR9366BD` adapter. The failure reproduced exactly
— zero side-1 RX bytes and exactly five active-RX UART errors for five frames —
which is what identified the adapter as the only constant.

### Why the earlier audit missed it

The UART-format audit tested "all four static RTS/DTR level combinations" and
concluded those control lines were "not selecting a working adapter direction
mode". That conclusion is right and the inference from it was wrong: an
RTS-controlled adapter cannot be driven by a static level in any of the four
combinations, because driver-enable must be asserted for the duration of each
transmission and released after it. The test that distinguishes the two cases
is a *dynamic* toggle, not a static level.

### Evidence

Three independent results, all with the production normal-polarity v2.2.0
image and unchanged wiring:

1. **Byte-pattern asymmetry.** Twenty bytes of each pattern into side 1 through
   `AR9366BD`: `0x00`, `0x55` and `0xAA` each produced 0 RX bytes and 1 UART
   error, while `0xFF` — the pattern that holds the line at mark for nine of
   every ten bit times — produced 40 RX bytes and 20 decoded frames. Reception
   only worked when the line barely had to be driven to space. **This also rules
   out reversed polarity**, which would give the opposite asymmetry: `0xFF`
   failing and `0x00` partially decoding.

2. **Only a dynamic RTS toggle moved data.** Five valid frames, four modes:

   | mode | side-1 rx bytes | frames | uart errors |
   |---|---:|---:|---:|
   | RTS static low | 0 | 0 | 5 |
   | RTS static high | 0 | 0 | 5 |
   | RTS raised per transmission | 126 | 14 | 9 |
   | RTS lowered per transmission | 0 | 0 | 5 |

   The byte and frame counts under the working mode are wrong because userspace
   RTS toggling over USB has millisecond jitter, so driver-enable asserts and
   releases late; the switching edges are received as extra bytes. That confirms
   the mechanism rather than providing a usable transport.

3. **A second adapter passed immediately.** FTDI FT232R serial `B003AHF1`
   (a different vendor's board, same chip) in the same wiring position, with no
   other change: 45 of 45 RX bytes, 5 of 5 frames decoded, and zero incomplete,
   oversized, queue-drop, parse, UART and transmission errors. Repeated three
   times. Every byte pattern above also decoded 20 of 20 with zero UART errors.

### Consequences

- `AR9366BD` must not be used with Router, RouterRS or PortalTestBench. None of
  them toggles adapter driver-enable, by design, so no host-side change makes
  this adapter work. Either replace it or program the FT232R's CBUS pin to
  `TXDEN` so direction control is hardware-timed.
- The 2026-08-24 electrical remediation advice above — check polarity, both
  conductors, common reference, termination and bias at the side-1 MAX3362 pins
  — was not the fault here and did not need to be carried out.
- The Portal home-switch findings in this report are unaffected: they were taken
  over the direct ST-Link VCOM path, which does not involve RS485 or either
  adapter.

### Repeater identity

The ESP32-C3 used for this section (Espressif USB serial/JTAG MAC
`f8:5b:1b:ed:8d:a4`) was found running an application built against ESP-IDF
v4.4.7 and dated 2024-03-05 — an Arduino-ESP32 2.0.x image predating the JSON
diagnostics console, which is why it answered no console command. Its full 4 MB
flash was captured before replacement:

- Recovered pre-existing 4 MB flash readback:
  `e17080d5d1f563c7da9ab6cf518069bceb0aef48b9446c251c736e08070c2265`

Production v2.2.0 (build `8799276081d5-dirty`, normal polarity on both sides)
was then built and uploaded, and verified live: `version` reports 2.2.0 at
115200 with `side1_inverted` and `side2_inverted` both false.

**A board whose console is silent is not necessarily faulty — check the flashed
image before drawing any conclusion from missing diagnostics.** The 1 Hz LED
heartbeat on GPIO10 distinguishes a running bridge from a dead one without
opening a port.

### Not yet done

Side 2 was left connected to the Portal branch throughout, and the repeater
stayed in `transparent` mode because no valid inner reply ever taught it a
local range. Addressed polling of IDs 1–9, the learned-range and filtering
checks, and the V3 batch/gap/54-Portal soak all remain outstanding on a
`B003AHF1`-class adapter.

## Adapter pair test and repeater isolation — 2026-08-25 (later)

### `B003ASAG` is a good adapter

Wired A-A/B-B to `B003AHF1` with nothing else on the pair, both adapters passed every
pattern in both directions at 115200 8N1, with **no local echo** on either:

| pattern | `B003ASAG` → `B003AHF1` | `B003AHF1` → `B003ASAG` |
|---|---|---|
| 20 × `0x00` | 20/20 | 20/20 |
| 20 × `0x55` | 20/20 | 20/20 |
| 20 × `0xFF` | 20/20 | 20/20 |
| COBS broadcast poll | 12/12 | 12/12 |

Passing `0x00` and `0x55` is the discriminator from the 2026-08-25 resolution above: an
adapter whose driver-enable follows RTS decodes only `0xFF`. `B003ASAG` therefore has
working automatic half-duplex direction control and is **not** an `AR9366BD`-class part.

Sustained upload-shaped traffic over the same pair — real 32-byte firmware frames with
their XOR-16 checksums, COBS-framed — was byte-exact at every pacing tried:

| profile | frames | result |
|---|---|---|
| 10 ms gap (`resilient`) | 200 | 9388/9388 bytes, 200/200 frames, byte-exact |
| 5 ms gap (`default`) | 200 | 9388/9388 bytes, 200/200 frames, byte-exact |
| 2 ms gap | 200 | 9388/9388 bytes, 200/200 frames, byte-exact |
| back-to-back, no gap | 300 | 14088/14088 bytes, 300/300 frames, byte-exact |

200 of 200 half-duplex round trips completed. (The ~56 ms median round trip is the test
harness's own read-timeout polling, not a property of the bus.)

**This proves the host transport only.** Two adapters on a two-node pair is not a
repeater store-and-forward hop, and it is not the Portal bootloader's 10 ms receive loop,
so it says nothing about either.

### The repeater is isolated on both sides

Measured before the adapters were paired, with the repeater running v3.0.0 and the branch
connected as normal:

| segment | direction | result |
|---|---|---|
| `B003ASAG` ↔ repeater side 1 | both | 0 bytes each way, 0 UART errors, all six DTR/RTS modes |
| repeater side 2 → Portal branch | 4,800 frames, `tx_errors=0` | module 1's `PA3` never left idle mark |
| repeater side 2, IDs 1–9 | polled twice each | nothing answered |
| repeater side 1, IDs 1–9 | polled twice each | nothing answered |

The `PA3` measurement is the decisive one and it is new: `rs485_line_probe`
(`PortalFlasher/crates/portal-swd/examples/`) samples `GPIOA->IDR` over SWD without
halting the core, so it observes the Portal's RS485 receive pin directly rather than
inferring from silence at one end of the bus. Over 9,380 samples in 8 s it recorded zero
edges and zero low samples on `PA3` while the repeater drove 4,800 frames at it. The same
run recorded `PB4` toggling 602 times, which is what makes "no edges" a measurement
rather than a broken tool.

`PA1` reading low throughout corroborates the pin mapping in `pins.md`: that is DE parked
in receive.

Since both adapters are now proven good, and the repeater drives its own transmitters
without error while neither of its peers hears anything, the remaining fault is in the
repeater's RS485 front end or its harnesses — **on both sides**. `Protocol.md` §7's
escalation applies: measure at the MAX3362 pins, not at a connector — transceiver VCC on
both sides, then continuity from each terminal (A, B, and the signal reference) through
to the transceiver pins.

One asymmetry worth carrying into that measurement: side 2's receiver picks up exactly one
corrupt byte per transmission when DE releases (`turnaround_events = 0`, so it arrives
*after* the driver lets go — a floating, unbiased line), while side 1 never glitches at
all. The two sides are in different electrical states; neither should be assumed from the
other.

### Still outstanding

Addressed polling of the branch, the learned-range and filtering checks, and the V3
batch/gap topology soak — unchanged from the 2026-08-25 resolution, and now blocked on the
repeater front end rather than on the adapter.

## Full-bench session — 2026-08-26 (evening)

Bench: portals 1,2,4,6,7 (IDs) on side 2; FTDI `B003AHF1` also on side 2 as a
sniffer; FTDI `B003ASAG` on side 1 as the host; ST-Link on ID 2; repeater index 1.

### The repeater's USB can wedge while the relay runs

The console and esptool were both silent ("No serial data received") while the
bridge demonstrably relayed — a delimited junk burst from side 2 appeared on
side 1, an undelimited one did not. The USB device itself had stopped taking
OUT data (`tcdrain` hung). `libusb_reset_device` on VID 303A PID 1001 revived
console and esptool without touching the hardware. Hold the console open in one
long-lived process; opening and closing it per command toggles DTR/RTS, which
is what produced the wedge.

### Side 1 is wired with its pair swapped, and that is now absorbed

With both UARTs at normal polarity, every `B003ASAG` frame arrived as exactly
one side-1 UART error and zero bytes, and relayed traffic reached the adapter
as an inverted-polarity garble (verified by UART simulation of the observed
bytes). The wiring stays; the firmware now runs a per-side polarity hunter
(`auto`): two errors with nothing decoded flips the UART inversion, two decoded
frames lock it, the locked value persists in NVS. On the bench the first two
polls were eaten, the third and every one after answered, and the lock survived
reboot and OTA.

### The upload corruption was the host's pacing, not the wire

A v6 application upload to ID 2 (100,744 B, 788 chunks) first ran at ~34 % chunk
loss, repaired by the map pass. The sampler timeline pinned it: clean for the
first ~230 frames, errors ramping from t+4 s to the end of the pass, and the
short repair pass clean again. The host was sleeping its 6 ms gap after
`write()` while a 176-byte frame takes 15 ms on the wire; once the OS buffer
filled, the FTDI was topped up packet-by-packet, its FIFO ran dry mid-frame and
TXDEN dropped — an undriven gap that the inverted side reads as a break. Fix:
`SerialPortDevice::transmit` now drains (`tcdrain`) before the gap. The repeater
also gained phantom-delimiter tolerance (a delimiter that closes nothing
COBS-valid is skipped once per frame) as the belt to that brace.

### Results after the fixes

- v6 application upload through the repeater: 867 frames, side 1 zero UART
  errors / zero phantoms / zero parse errors, map complete on first read, no
  repair, `verify` CRC match, bootloader `drops: 0`, 28 s.
- In-band bootloader replacement (§10.5) on ID 2 through the repeater: 125
  ACKed frames, zero errors, confirmed `Bootloader v6`, 11 s.
- Repeater control plane over the inverted side: `status` (long reply) and
  `set-polarity` verbs answered; driver-enable is now timed by the UART
  peripheral (`de-mode hw`), which removed the milliseconds-deaf turnaround.
- Repeater self-OTA over RS485 through side 1: 707/707 chunks, 0 parse
  failures, 0 repairs, ~50 s. Four consecutive updates committed but never
  rebooted: the `ota-end` reply's driver release left one debris byte in the
  repeater's own receiver, and `ota-boot`, arriving 5–20 ms later, was glued to
  it and lost (`ctrl_seen` froze across the boot window while a hand-sent
  `ota-boot` minutes later rebooted instantly). Fixed in the bridge — bytes
  arriving in a 3 ms post-transmit shadow with no frame in progress are
  discarded — plus a 50 ms host pause before `ota-boot` for bridges that
  predate the fix. The updated image then rebooted, self-validated on traffic
  and reported `confirmed`.
- Portal ID 1 answers nothing, on the direct side-2 adapter as well as through
  the repeater, while 2, 4, 6, 7 answer everything. That board needs its own
  investigation; it is not an RS485 or repeater fault.
