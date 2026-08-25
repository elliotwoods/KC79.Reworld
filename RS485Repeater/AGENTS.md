# RS485Repeater agent guide

This directory is the ESP32-C3 Reworld V3 frame router. Before hardware work, read:

1. `../Protocol.md`, especially **Installation topology versions**, **Operating and commissioning
   a V3 repeater**, **Physical & electrical layer**, and **Firmware update / bootloader protocol**.
2. `README.md` for build, recovery, port identity, and diagnostics.
3. `FIELD_REPORT_2026-08-24.md` only for the evidence and unresolved state of that field session.

## Non-negotiable topology facts

- V1 and V2 remain supported and have no ESP32 repeaters.
- V3 has one shared outer host bus and six repeaters, each serving one isolated nine-Portal branch.
- Repeater side 1 is always the shared host/WaveShare bus; side 2 is always the local Portal bus.
- Both sides use 115200/8N1. Production v2.2.0 has logical inversion off on both sides.
- Firmware-update broadcasts must remain fail-open and reach every branch.
- The repeater learns its local contiguous nine-ID block from a valid side-2 reply. Unknown traffic
  is fail-open; a conflicting block puts it in conflict/transparent mode.

## Hardware and serial-port safety

- Identify serial ports by USB VID/PID/serial or an identity query, never by a transient
  `/dev/cu.usbmodem*` number.
- An FTDI USB-RS485 port carries COBS/MessagePack wire frames. The ST-Link VCOM is direct Portal
  ASCII serial. The Espressif USB port is the repeater JSON console. Do not interchange them.
- Disable USB-RS485 local echo before judging replies. Router/RouterRS assume the adapter or gateway
  performs automatic half-duplex direction control; they do not toggle DE with RTS.
- A/B labels are not portable between vendors. Prove polarity with a complete frame or a scope.
- RS485 still needs a valid common-mode reference. Do not remove the designed ground/reference
  unless the link is intentionally galvanically isolated.
- Never interpret a Portal startup LED failure as a reason to flash PortalFW until a direct
  VCOM/SWD check distinguishes firmware from mechanical/optical failure.

## Safe test order

1. Query repeater USB `version`; confirm version, baud, and both polarity flags.
2. Run `reset-counters` (preserves learned range) or `relearn` only when relearning is intended.
3. Send one known-good, non-motion frame and inspect counters. Do not start with burst discovery.
4. Poll expected IDs sequentially; require complete source-correct replies and zero error/drop
   counters.
5. Verify learned range and filtering, then run the V3 batch/gap/full-topology soak.

For an upstream-only electrical test, disconnect side 2 physically or use a deliberately filtered
valid frame after the local range is learned. Remember that malformed traffic is fail-open and can
reach side 2. Stop any Portal routine with Escape before starting RS485 measurements.

If normal polarity gives UART errors and inverted polarity gives only partial/incomplete data,
neither polarity passes. Check MAX3362 A−B, common mode, tied `DE`/`RE#`, and `RO` at the ESP RX pin;
do not choose the setting with fewer errors.

## Firmware changes and restoration

- Back up the full 4 MB flash before replacing an unknown field image; preserve its SHA-256.
- Temporary polarity builds are allowed for controlled diagnosis, but restore both `platformio.ini`
  and the installed production image to normal polarity unless the user explicitly authorises a
  documented installation override.
- Do not modify PortalFW to solve repeater topology, pacing, or upstream electrical faults.
- Preserve unrelated dirty-tree changes and field artifacts.

## Verification

```sh
~/.platformio/penv/bin/pio test -d RS485Repeater -e native
~/.platformio/penv/bin/pio run -d RS485Repeater -e repeater
```

All 10 native BridgeCore tests and the embedded release build must pass. Check the live USB
`version` response after upload so the device's polarity agrees with `platformio.ini`.
