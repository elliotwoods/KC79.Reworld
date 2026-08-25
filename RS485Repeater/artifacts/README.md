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
