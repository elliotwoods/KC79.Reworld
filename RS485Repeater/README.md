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

For Reworld V3, side 1 is the shared host/WaveShare bus and side 2 is one local nine-Portal branch.
The router starts transparent, learns its local contiguous nine-ID range from the first valid
inner reply, and then filters non-local unicasts and non-intersecting keyframes. Unknown broadcasts
and all firmware-update frames remain transparent. A reply from a conflicting ID block returns the
unit to fail-open conflict mode until reboot or `relearn`.

An earlier field setup used a host-side wiring orientation that was temporarily compensated with
UART inversion in repeater v2.1.6. The V3 design and v2.2.0 production build use normal polarity on
both sides (`REPEATER_SIDE1_INVERT=0` and `REPEATER_SIDE2_INVERT=0`). A/B labels are not consistent
between RS485 vendors, so neither labels nor an absence of UART errors proves polarity. Do not
persist a software-inversion override unless a scope or complete known-good frame proves that the
installed differential pair is reversed and the exception is recorded for that installation.

Because each MAX3362 has RE# tied to DE, its RO pin floats whenever that side transmits. The
firmware enables an internal pulldown for the inverted UART and a pullup for the normal UART so
those high-impedance intervals cannot be misreported as UART break/frame errors.

The ESP32 UART may still raise a hardware event at the exact RE#/DE turn-around. Events received
while that side is deliberately transmitting are reported separately as `turnaround_events`;
`uart_errors` only counts errors while the receiver is active.

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

Replies are single-line JSON. `side1` means UART0/GPIO20+21 and `side2` means UART1/GPIO6+4; each
status object explicitly reports whether its RX/TX polarity is inverted. It also reports routing
mode/range, filter counts, queue depths/high-water marks/drops, parse failures, incomplete and
oversized frames, UART errors, and transmission errors. Healthy V3 traffic learns the expected
range and leaves every error/drop counter at zero.

`reset-counters` does not forget the learned range. `relearn` does: it returns the router to
transparent mode until a valid inner reply identifies the local nine-ID block.

## Commissioning and upstream diagnosis

1. Disable local echo on the USB-RS485 adapter and confirm it provides automatic half-duplex
   direction control. Router/RouterRS do not toggle adapter DE through RTS.
2. Query `version`, confirm the expected polarity flags, then run `reset-counters`.
3. Send one known-good non-motion frame at 115200/8N1 and wait before sending another. Do not use a
   burst discovery as the first electrical test; it can fill the four-frame queue.
4. Poll the nine expected IDs individually. Accept only complete replies with the correct sources,
   the correct learned range, empty queues, and zero UART/incomplete/oversized/parse/drop/conflict/
   TX errors.
5. Then test filtering and the full V3 batch/gap/54-Portal topology described in `Protocol.md`.

If neither normal nor inverted UART polarity produces a complete frame, do not keep changing baud
or Portal firmware. Every production participant uses 115200/8N1. Measure A−B and common-mode
voltage at the actual side-1 MAX3362 pins, check tied `DE`/`RE#` is LOW during host transmission,
and inspect `RO` at GPIO20. Local adapter echo is only the adapter receiving its own transmission;
it is not a Portal reply. Provide the designed signal reference unless both ends are intentionally
galvanically isolated.

Keep run-specific captures and conclusions in `FIELD_REPORT_2026-08-24.md`; keep reusable topology
and protocol behavior in `Protocol.md`.
