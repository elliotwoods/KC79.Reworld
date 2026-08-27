# KC79 RS485 repeater

The connected-panel investigation and acceptance results are recorded in
[`FIELD_REPORT_2026-08-24.md`](FIELD_REPORT_2026-08-24.md).

Firmware for the ESP32-C3-WROOM-02U-N4 bridge between two MAX3362 RS485 segments. Both sides use
115200 baud, 8 data bits, no parity and one stop bit. The project pins the same Arduino-ESP32
3.3.11 / ESP-IDF 5.5.5 toolchain found in the recovered field image.

The old v2.0 sketch switched driver-enable and waited for TX completion for every byte. Version
2.2.0 is a bounded store-and-forward router: it receives a complete COBS frame (`0x00`) before
asserting the destination driver once for the whole frame. A 2 ms idle fallback discards a
truncated stream, and oversized or queue-overflowed frames are dropped atomically rather than
forwarded partially.

Version 3.0.0 keeps all of that and adds a **control plane**: the repeater now has an address, can
be queried and configured over RS485, can sweep its own branch for positions, and can be given new
firmware in-band. See [`../Protocol.md`](../Protocol.md#11-the-repeater-control-plane) §11 and §12
for the wire contract. Three changes to the existing bridge come with it:

- `MAX_FRAME_BYTES` drops from 8192 to 2048 and `FRAME_QUEUE_DEPTH` rises from 4 to 16 — the same
  total queue RAM, but the worst-case blocking write falls from 711 ms to 178 ms, and the queue
  absorbs the keyframes that arrive while a branch sweep is running. The nine-entry keyframe batch
  V3 uses measures 225 framed bytes and the largest configurable one (54 entries, full-range
  positions and velocities) measures 1172, both pinned by a `router-proto` test so a future change
  that would overflow the limit fails there rather than silently dropping frames in the field.
- The learned nine-ID range is persisted to NVS and restored at boot, so a cold start no longer
  fails open and floods the branch with all 54 unicasts until an inner reply happens to arrive.
- The loop task subscribes to the 5 s task watchdog, which the Arduino core leaves off.

For Reworld V3, side 1 is the shared host/WaveShare bus and side 2 is one local nine-Portal branch.
The router starts transparent, learns its local contiguous nine-ID range from the first valid
inner reply, and then filters non-local unicasts and non-intersecting keyframes. Unknown broadcasts
and all firmware-update frames remain transparent. A reply from a conflicting ID block returns the
unit to fail-open conflict mode until reboot or `relearn`.

## Polarity is decided on the wire, per side

A/B labels are not consistent between RS485 vendors, and a pair landed the wrong way round is a
hardware fact an installation may not be able to change. Measured on the bench 2026-08-26: the
host adapter on side 1 was wired with its pair swapped relative to the repeater's transceiver, so
with both UARTs at normal polarity every host frame arrived as exactly one UART error and zero
bytes, and everything relayed upstream reached the host as an inverted-polarity garble.

Each side therefore runs a **polarity hunter** (`lib/BridgeCore/src/Polarity.*`), and the ESP32
inverts RXD and TXD in hardware when it says so:

- `auto` (the default): a side that has never decoded a frame at its current polarity flips after
  two UART errors, at most once per 200 ms; once two frames decode it **locks**, and the proven
  polarity is written to NVS so the next boot starts there. A locked side only re-hunts after
  twelve errors with no valid frame among them -- more than any glitch burst seen on the bench,
  less than a second of a host talking at the wrong polarity.
- `normal` / `inverted`: pinned, for a documented installation override.

Bytes and errors arriving within 3 ms of that side's own driver letting go are the turn-around
glitch of a floating line, not traffic: they are excluded from the evidence and reported as
`turnaround_events`, so a firmware upload -- thousands of frames out, nothing back for minutes --
cannot talk a working side into flipping. `uart_errors` counts only errors outside that window.

`REPEATER_SIDE1_INVERT` / `REPEATER_SIDE2_INVERT` in `platformio.ini` are now only the starting
point for a unit that has never locked. The USB `polarity` command reports both sides
(`mode`, `inverted`, `locked`, `flips`, the evidence counters); `polarity <1|2> <auto|normal|inverted>`
sets and persists a mode, and the control-plane verb `set-polarity` does the same over RS485.
`status` carries `inverted`/`polarity`/`locked`/`flips` per side.

Because each MAX3362 has RE# tied to DE, its RO pin floats whenever that side transmits. The
firmware biases the ESP RX pin to that UART's idle level -- pulldown when inverted, pullup when
not -- so those high-impedance intervals cannot be misreported as UART break/frame errors.

## Driver-enable is timed by the UART

By default (`de-mode hw`) each side's DE/RE# is driven by the UART peripheral in its RS485
half-duplex mode, with RTS as the enable: the driver drops the instant the stop bit is out and the
receiver is back before a fast responder's first byte. The old GPIO path, where the loop waited for
`uart_wait_tx_done` and then dropped the pin, left the receiver deaf for milliseconds after every
relayed frame and lost the head of the reply behind it. `de-mode sw` persists the GPIO path as a
fallback and takes effect at once; `status` reports `de_hardware`.

## A phantom delimiter is skipped, once

A COBS delimiter that closes something which is not a COBS frame is taken -- once per frame -- to
be a phantom rather than an end, and the frame stays open. A host adapter whose FIFO runs dry lets
its driver go between two USB packets of one frame; on an inverted pair that undriven gap reads as
a break, and a break is a zero. If the next delimiter does not close a frame either, the bytes were
corrupt rather than split and the frame is given up without swallowing the one behind it. Counted
as `phantom_delimiters` per side. (The host-side cause -- pacing frames by sleeping after a write
that has not reached the wire -- is fixed in RouterRS by draining the port before the gap; the
repeater's tolerance is the belt to that brace.)

## Turnaround debris cannot open a frame

When one of the repeater's own transmissions ends and its driver lets go, the receiver comes back
to a line nobody is driving and samples a byte of debris. Left alone, that byte opened a partial
frame, and any host request arriving within the 20 ms incomplete-frame window was glued to it and
died -- which is exactly what `ota-boot`, sent milliseconds after the `ota-end` reply, did on four
consecutive bench updates. Bytes arriving inside a 3 ms post-transmit shadow while no frame is in
progress are now discarded (`shadow_bytes` per side); a frame already being received is never
touched. The same shadow keeps those bytes out of the polarity evidence.

## Topology, pins, and serial-device identity

The protocol contract and the V1/V2-versus-V3 topology are in
[`../Protocol.md`](../Protocol.md#installation-topology-versions). Side 1 is always the shared
host/WaveShare bus; side 2 is always the local nine-Portal branch. The ESP32 pins are:

| | RX from MAX3362 `RO` | TX to `DI` | tied `DE`/`RE#` |
|---|---:|---:|---:|
| side 1 / UART0 | GPIO20 | GPIO21 | GPIO7 |
| side 2 / UART1 | GPIO6 | GPIO4 | GPIO5 |

Both UARTs are explicitly 115200 baud, 8 data bits, no parity, one stop bit. Tied `DE`/`RE#` is
LOW to receive and HIGH to transmit.

Do not select ports by `/dev/cu.usbmodem*` suffix alone; reconnecting USB can change it. Identify
the USB parent or make a harmless identity query. The 2026-08-24 bench had three different serial
functions attached simultaneously:

- FTDI FT232R serial `AR9366BD`: USB-RS485 wire transport
- ST-Link V2-1 VCOM: direct Portal ASCII serial; `v` prints the Portal version
- Espressif USB Serial/JTAG, MAC `f8:5b:1b:f4:18:ec`: repeater console; `version` returns JSON

The ST-Link VCOM bypasses RS485 and the repeater. It can prove a Portal firmware/mechanical fault
independently, but sending a command there is not an RS485 test.

## Build and test

PlatformIO is installed in `~/.platformio/penv/` on the development machines used by this repo:

```sh
~/.platformio/penv/bin/pio test -d RS485Repeater -e native
~/.platformio/penv/bin/pio run -d RS485Repeater -e repeater
```

The first ESP32 build downloads the pinned pioarduino platform. The application binary is
`RS485Repeater/.pio/build/repeater/firmware.bin`.

## Back up, flash, recover

Always capture the entire 4 MB device before replacing a field image:

```sh
~/.platformio/penv/bin/esptool --port /dev/cu.usbmodem21401 read-flash 0 0x400000 repeater-backup.bin
shasum -a 256 repeater-backup.bin
~/.platformio/penv/bin/pio run -d RS485Repeater -e repeater -t upload --upload-port /dev/cu.usbmodem21401
```

To recover, write the full backup from offset zero with esptool. This overwrites all partitions,
so use it only for an explicit rollback:

```sh
~/.platformio/penv/bin/esptool --port /dev/cu.usbmodem21401 write-flash 0 repeater-backup.bin
```

## USB diagnostics

USB CDC is independent of both RS485 UARTs. Open `/dev/cu.usbmodem*` at 115200 and send one of:

- `status`
- `version`
- `reset-counters`
- `relearn`
- `index` — report the provisioned repeater index and MAC
- `set-index N` — set it to 1..6, or 0 to unprovision
- `ota-state` — running slot, session progress, whether relaying is paused
- `rollback` — revert to the other slot and reboot
- `polarity` — both sides' polarity mode, current inversion, lock state and evidence;
  `polarity <1|2> <auto|normal|inverted>` sets and persists one side's mode
- `de-mode` — who drives DE/RE#; `de-mode hw|sw` sets and persists it
- `idle-timeout [us]` — the unterminated-frame timeout, adjustable live
- `tx-test <1|2> [count] [portal-id]` — put inert frames, or a unicast poll, on one bus from the
  repeater itself

The console deliberately stays a diagnostic surface: everything bulk goes over RS485. `set-index` is
here because a repeater has to be given its identity at commissioning, before it has one to be
addressed by, and `rollback` is here because that is the one operation you want available when the
wire is exactly what is not working.

Replies are single-line JSON. `side1` means UART0/GPIO20+21 and `side2` means UART1/GPIO6+4; each
status object explicitly reports whether its RX/TX polarity is inverted. It also reports routing
mode/range, filter counts, queue depths/high-water marks/drops, parse failures, incomplete and
oversized frames, UART errors, and transmission errors. Healthy V3 traffic learns the expected
range and leaves every error/drop counter at zero.

`reset-counters` does not forget the learned range, and it rebases rather than persuades the
polarity hunters. `relearn` clears the range.

### If the USB console is silent

A silent console is not a dead repeater. On 2026-08-26 the C3's USB-JTAG stopped accepting bytes
(`tcdrain` hung, esptool got "No serial data received") while the relay kept running. Prove the
relay first: send a byte burst *without* a COBS delimiter from one side and the same burst *with*
one, and watch the other side -- only the delimited one is relayed. Then reset the USB device from
the host without unplugging it (`libusb_reset_device` on VID 303A PID 1001); after that both the
console and esptool answer. Hold the console open in one long-lived process rather than opening
and closing it per command: the C3 resets on a DTR-low/RTS-high transition, and with DTR
deasserted it NAKs writes, which is how the wedge was produced in the first place.

## Commissioning and upstream diagnosis

1. Disable local echo on the USB-RS485 adapter and confirm it provides automatic half-duplex
   direction control. Router/RouterRS do not toggle adapter DE through RTS.

   **Prove that, do not assume it.** An adapter whose driver-enable follows RTS never drives the
   bus at all under any static RTS/DTR level, so it presents as an electrical fault: edges but no
   decodable byte, and every continuity, polarity and termination check passing. Send twenty bytes
   each of `0x00`, `0x55` and `0xFF` and read side-1 counters. An adapter with working automatic
   direction control decodes 20 of 20 for every pattern. An adapter without it decodes nothing
   except under `0xFF`, which holds the line at mark nine bit times in ten — an asymmetry that is
   diagnostic, and is the opposite way round from reversed polarity. Confirmed good: FT232R serials
   `B003AHF1` and `B003ASAG`. Confirmed unusable: FT232R serial `AR9366BD`. See the 2026-08-25
   resolution in [`FIELD_REPORT_2026-08-24.md`](FIELD_REPORT_2026-08-24.md).

   The cheapest way to prove an adapter is against another adapter, not against the bridge: wire
   two of them A-A/B-B and send both ways. That removes the repeater, the branch and every
   firmware from the experiment, so a pass means the remaining fault is downstream of the host
   entirely. It is worth doing *before* the byte-pattern test above, because a silent bridge and
   a silent adapter look identical from the host.
2. Query `version`, confirm the expected polarity flags, then run `reset-counters`.
3. Send one known-good non-motion frame at 115200/8N1 and wait before sending another. Do not use a
   burst discovery as the first electrical test; it can fill the four-frame queue.
4. Poll the nine expected IDs individually. Accept only complete replies with the correct sources,
   the correct learned range, empty queues, and zero UART/incomplete/oversized/parse/drop/conflict/
   TX errors.
5. Then test filtering and the full V3 batch/gap/54-Portal topology described in `Protocol.md`.
6. Provision the identity: `set-index N` over USB, matching the nine-ID block this unit serves.
   An unprovisioned unit is still reachable over the wire by MAC, and answers as address `-2`, but
   it cannot be addressed by index until this is done.

## Repeater OTA runbook

The full contract is `../Protocol.md` §12. Operationally:

1. **Back up the running 4 MB flash first** and record the SHA-256 in `artifacts/README.md`. This
   applies to the USB install of v3.0.0; after that, the previous image lives in the other OTA slot.
2. Install v3.0.0 over USB once, on every unit. v2.2.0 has no OTA, so the first rollout cannot be
   in-band. This is also the only moment the partition table could be changed, should a factory
   recovery slot ever be wanted.
3. Read `status` from every repeater and confirm `proto` before using any OTA verb. A mixed fleet
   degrades per repeater, not fleet-wide.
4. Update **one repeater at a time** by default. Broadcast is faster but pauses all six bridges at
   once and blacks out the whole installation.
5. **Budget the time honestly:** about 33 s per repeater, so roughly 3.5 minutes for a rolling fleet
   update, or 45–90 s for a broadcast pass including gap repair. Not a 45-second window.
6. A new image resolves its own pending-verify state within about 30 seconds on local evidence. The
   host never has to confirm it, and must not be relied on to — a rack that powers up before the
   show PC would otherwise revert every morning.

If neither normal nor inverted UART polarity produces a complete frame, do not keep changing baud
or Portal firmware. Every production participant uses 115200/8N1. Measure A−B and common-mode
voltage at the actual side-1 MAX3362 pins, check tied `DE`/`RE#` is LOW during host transmission,
and inspect `RO` at GPIO20. Local adapter echo is only the adapter receiving its own transmission;
it is not a Portal reply. Provide the designed signal reference unless both ends are intentionally
galvanically isolated.

Keep run-specific captures and conclusions in `FIELD_REPORT_2026-08-24.md`; keep reusable topology
and protocol behavior in `Protocol.md`.
