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
