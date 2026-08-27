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
- Both sides use 115200/8N1. Polarity is decided per side on the wire (`Polarity.*`, mode `auto`
  by default, persisted in NVS); the build flags are only a starting point. A pair landed the wrong
  way round is absorbed, not diagnosed.
- Firmware-update broadcasts must remain fail-open and reach every branch.
- The repeater learns its local contiguous nine-ID block from a valid side-2 reply. Unknown traffic
  is fail-open; a conflicting block puts it in conflict/transparent mode.

## The repeater control plane (v3.0.0)

- Repeater-plane frames use **envelope target 0** with the repeater address in the body. This is not
  cosmetic. `shouldForward` drops side-1 `target == 0` unconditionally in every routing mode, so a
  unit running v2.2.0 ignores control traffic; an unrecognised negative target would instead
  fail-open and relay it — a 300 kB OTA image onto nine Portals. Do not "tidy" the address into the
  envelope. The native test `test_host_addressed_frames_are_dropped_in_every_routing_mode` pins this.
- A repeater-plane frame must never reach a Portal branch, on new or old firmware.
- Any verb that solicits a reply is unicast-only. Six answers to one broadcast collide.
- The repeater index lives in NVS and is **not** derived from the learned range: a unit with a dead
  branch never learns one, which is exactly when it most needs to be addressable. MAC addressing is
  the escape hatch and must keep working.
- Rollback keys on local evidence of malfunction — a boot-loop counter, or positive proof that
  frames decoded — never on host contact. A host-confirm gate would revert the fleet every morning a
  rack powered up before the show PC, and `esp_ota_begin` refuses to run while an image is pending
  verification, so it would also lock out the fix.
- `ota-begin` erases the slot and must be acknowledged before the host streams. `CONFIG_UART_ISR_IN_IRAM`
  is not set and cannot be, so the UART ISR cannot run during an erase and inbound bytes are lost.
- Guard every `esp_ota_write_with_offset` against "no open session". Assertions are enabled in this
  build and IDF asserts the partition was erased, so a stray chunk panics rather than erroring.

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

If `polarity` shows a side flipping repeatedly (`flips` climbing, never `locked`), neither polarity
decodes. Check MAX3362 A−B, common mode, tied `DE`/`RE#`, and `RO` at the ESP RX pin; do not pin the
setting with fewer errors.

## Host pacing

A serial `write()` that has returned is not a frame on the wire. RouterRS drains the port
(`tcdrain`) before it sleeps the inter-frame gap, and every host tool must: a host that queues
faster than 115200 baud fills the OS buffer within seconds, after which the adapter is fed one USB
packet at a time, its FIFO runs dry mid-frame and its driver drops -- invisible on a pair at the
receiver's polarity, fatal on an inverted one. Measured 2026-08-26: the first ~230 frames of an
upload were clean, every frame after that was hit, and the short repair pass was clean again.

## Firmware changes and restoration

- Back up the full 4 MB flash before replacing an unknown field image; preserve its SHA-256.
- Do not pin a side's polarity (`polarity <side> normal|inverted`) except as a documented
  installation override; `auto` is the production setting and the proven value persists on its own.
- Do not modify PortalFW to solve repeater topology, pacing, or upstream electrical faults.
- Preserve unrelated dirty-tree changes and field artifacts.

## Verification

```sh
~/.platformio/penv/bin/pio test -d RS485Repeater -e native
~/.platformio/penv/bin/pio run -d RS485Repeater -e repeater
cd RouterRS && cargo test -p router-proto -p router-link -p router-core
```

All five native suites (`test_bridge`, `test_control`, `test_ota`, `test_polarity`, `test_snapshot`)
and the embedded release build must pass. Check the live USB `version` response after upload so the device's polarity
agrees with `platformio.ini`.

The control plane, OTA session and snapshot engine live in `lib/BridgeCore` behind injectable clocks
and an injectable flash target specifically so they run in `[env:native]`. Keep them there: a state
machine that only exists in `src/main.cpp` cannot be tested at all.
