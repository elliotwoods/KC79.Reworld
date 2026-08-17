# Native protocol tests

Host-side tests that compile the **real** `msgpack-arduino` sources — the submodule at
`../../PortalFW/lib/msgpack-arduino`, on the **non-Arduino** code path, which is the path the
RS485 bootloader builds against. A passing run is evidence about the parser that actually ships,
not about a re-implementation of it.

```powershell
powershell -File run.ps1
```

Every `*_test.cpp` in this directory is built and run. No PlatformIO, no ArduinoFake, no board.

## Why MSVC

PlatformIO's `native` platform needs a host `gcc`/`g++`, which is not installed on the Windows
bench machines here. `run.ps1` finds the Visual Studio C++ toolchain through `vswhere` instead.
With gcc available the same sources build directly:

```sh
g++ -std=c++17 -I../../PortalFW/lib/msgpack-arduino/src \
    ../../PortalFW/lib/msgpack-arduino/src/msgpack/*.cpp \
    ../../PortalFW/lib/msgpack-arduino/src/msgpack/lwrb.c \
    platform_shim.cpp fw_frame_offset_test.cpp -o fw_frame_offset_test
```

`platform_shim.cpp` supplies the handful of symbols the library declares but does not define; see
its comment for which ones are real (`msgpack::delay`, mirrored from the bootloader's
`msgpack_HAL.cpp`) and which exist only because MSVC resolves symbols before `/OPT:REF` runs.

## `fw_frame_offset_test.cpp`

Answers a specific long-standing worry: *does something in the firmware-update path narrow the
frame offset to 16 bits, capping the uploadable image at 64 kB?*

The application bank is `0x08006000..0x08020000` = 106,496 bytes, so the last frame of a full
image starts at offset 106,464 — well past 65,535. The test replays
`FWUpdateApp::processIncoming`'s exact parse sequence (`nextDataTypeIs(Map)` → `readMapSize` →
`readInt<uint32_t>` → `readBinarySize` → `readRaw`) over a real `COBSRWStream`, for:

- the last frame of a completely full bank, with its wire bytes pinned (`CE 00 01 9F E0`);
- every msgpack width boundary — fixint, uint8, uint16 and the uint32 crossing at 65,536;
- **all 3,328 frame offsets** of a full image, so nothing wraps or aliases anywhere in the bank;
- a deliberately 16-bit-narrowed key, asserting the parser reports the truncated value — which is
  what keeps the checks above from being vacuous.

**Result: no truncation anywhere in the path.** Frame offsets are 32-bit end to end. See
`protocol-hardening.md` §7 for what the real constraints turned out to be.

One thing this cannot tell you: it exercises the submodule's current **256-byte** COBS decode
buffer. The bootloader fielded today carries an older snapshot of the library with a **64-byte**
buffer, in which a 32-byte frame at a uint32 offset occupies 47 of those 64 bytes — it fits, with
no margin. That is an argument for unifying on the submodule, and it is why the acceptance test
for the RS485 path is still an on-bench upload of a >64 kB image followed by an SWD readback.
