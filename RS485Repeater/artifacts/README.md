# Field recovery artifacts

The untracked file `esp32-repeater-pre-v2.1.0-20260824.bin` is the complete 4 MB flash readback
captured immediately before installing repeater v2.1.0 on 2026-08-24.

- SHA-256: `fff905970c6a308a7c725e24a9532b975aa20890d5b7df490ab88221c22f9950`
- Flash offset: `0x00000000`
- Length: `0x00400000` bytes

Verify before using it for recovery:

```sh
shasum -a 256 RS485Repeater/artifacts/esp32-repeater-pre-v2.1.0-20260824.bin
```

## Reworld V3 router v2.2.0

`esp32-repeater-v2.2.0-20260824.factory.bin` is the combined bootloader,
partition table, and application image for installation at flash offset
`0x00000000`.

- Factory image SHA-256: `14a13fc1a9826672c4cce05a19a704c710d184f3e87235187939f9c3b3ce4d68`
- Application-only SHA-256: `e8fd55d32703e21e3719d34a7495bfcf54260d0e0f05a8009c3d481061d919ef`
- Firmware version: `2.2.0`
- Connected ESP32 MAC validated: `f8:5b:1b:f4:18:ec`

## Pre-v3.0.0 capture, 2026-08-25

The untracked file `esp32-repeater-pre-v3.0.0-20260825.bin` is the complete 4 MB flash readback
taken immediately before the first v3.0.0 install, on the bench unit whose console reported
version `2.2.0`, build `8799276081d5-dirty`, both sides non-inverted.

- SHA-256: `712f824bd8049fc7484237876fe2cd510e07c10442cd2e17c816ee55fc1231ff`
- Flash offset: `0x00000000`
- Length: `0x00400000` bytes
- ESP32-C3 MAC: `f8:5b:1b:ed:8d:a4` (QFN32 rev v0.4)

This is the unit used for the RS485 field-update bench work, and it is a *different* board from
the `f8:5b:1b:f4:18:ec` recorded for the v2.2.0 factory image above.

```sh
~/.platformio/penv/bin/esptool --port /dev/cu.usbmodem8411301 \
    write-flash 0 RS485Repeater/artifacts/esp32-repeater-pre-v3.0.0-20260825.bin
```
