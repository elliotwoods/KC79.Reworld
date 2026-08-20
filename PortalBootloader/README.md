# PortalBootloader

The RS485 field-update bootloader for KC79 Portal boards. 24 kB at `0x08000000`; the application
sits above it at `0x08006000`.

```powershell
pio run -e bootloader          # what goes on a board
```

or, from the repository root and on either platform — it finds `pio` wherever the installer put it,
and checks the image against the 24 kB bank and its reset vector before reporting success:

```sh
node tools/build-firmware.mjs --env bootloader
```

The platform is pinned (`platform = ststm32@19.6.0`) and has to be. Unpinned, PlatformIO resolves
whatever release a machine already carries: on one with 17.6.0 installed that is
`framework-stm32cubeg0@1.5.0`, whose `HAL_UART_Transmit` takes a **non-`const`** `uint8_t *pData`,
so `Logger.cpp` and `SerialStream.cpp` fail to compile against sources that pass a `const uint8_t *`.
The build succeeded on the machine that happened to have 19.x and nowhere else. See the repository
[`README.md`](../README.md) for the build and packaging workflows this project feeds.

`PortalFlasher` finds `.pio/build/bootloader/firmware.bin` automatically and prefers it over the
committed reference image from that point on.

| | |
|---|---|
| `cube-import/` | The STM32CubeIDE project, imported unmodified. The build compiles straight out of it. |
| `reference/` | The 2023 image every fielded board runs, and its `.elf`. |
| `test-native/` | 47 checks over the frame-offset arithmetic, built with MSVC on the host. |
| `bootloader.ld` | The linker script, with the bank length corrected. |

## The port is a build configuration, not a rewrite

`platformio.ini` points `src_dir` straight at `cube-import/Core` and adds two include paths.
Nothing was moved into a `src/` directory and no `#include` was rewritten, so `git diff` against
the import commit is exactly the set of changes porting required — one source edit, and it is a
bug fix.

Four things the build settled, none of them guessable from reading:

**`board_build.stm32cube.custom_config_header = yes` is required and silent when missing.**
Without it the framework compiles against its own `stm32g0xx_hal_conf.h` and the project's — which
is where it decides which HAL modules exist at all — is ignored. The build still succeeds.

**The linker script said `LENGTH = 28K` for a 24 kB bank.** `bootloader.ld` is the imported script
with that one number corrected. At 22,708 bytes the difference has never mattered; a link that is
permitted to run 4 kB into the application bank is not something to keep once it has been seen.

**`syscalls.c` and `system_stm32g0xx.c` are excluded from the build.** The framework supplies both
from CMSIS, generated from the same ST template, and compiling both is a duplicate-symbol link
error. `sysmem.c` stays: it is the real `_sbrk`, where libnosys's only fails.

**`assert(lwrb_init(...))` did not compile, and that was lucky.** CubeIDE's include list happened
to pull `assert` in transitively and PlatformIO's does not. The declaration was the smaller
problem: `assert` compiles to *nothing* under `NDEBUG`, so in a release build the `lwrb_init` call
disappears along with the check, leaving a `SerialStream` whose ring buffer was never initialised —
in the firmware that receives updates over RS485. It survived because the buffer is a member of a
statically-allocated object and therefore starts zeroed, which is close enough to initialised that
the failure would have been intermittent rather than immediate.

## How it compares to the 2023 reference

Bit-for-bit was never available: that image was linked by GCC 10.3 under CubeIDE, and PlatformIO's
`ststm32@19.6` resolves GCC **7.2.1** for `framework-stm32cube` — older, not newer, which the plan
had the wrong way round. So the gate is behavioural equivalence, structurally diffed.

| | reference | `bootloader` |
|---|---|---|
| text | 22,692 | 19,428 |
| data | 16 | 12 |
| bss | 2,884 | 2,880 |
| defined symbols | 370 | 323 |
| initial SP | `0x20009000` | `0x20009000` |
| banner | `Bootloader v4` | `Bootloader v5` |

(The `text`/reset-vector numbers above are after the fixes in the next section — the defined-symbol
count is unchanged by them, since every fix edits an existing function rather than adding one.)

The build deliberately uses `-fno-lto`, both so every function remains structurally comparable
and because GCC 7 LTO mislinks the interrupt vectors as described below.

**newlib.** The library-only symbols in the reference but not in `bootloader` are newlib internals
— `__sfp`, `__swsetup_r`, `_fflush_r`, `fiprintf`, `sbrk_aligned` and their
relatives, plus a handful of function-static counters. Not one is project code, HAL, or msgpack.
The reference linked full newlib stdio; PlatformIO defaults to `nano.specs`, and the ~3.3 kB
difference is that machinery. Nothing here formats a float, which is nano printf's one real
limitation.

The vector table agrees exactly on the initial stack pointer, the reset vector is inside the bank
with the Thumb bit set, and the `Bootloader v4` banner survives into the image — which is what
`PortalFlasher`'s readback scrapes to identify a board.

## Fixes layered on top of the import

Found by reading the source while answering "can the bootloader be improved at all,
without breaking compatibility" — none change the wire protocol, so a mixed fleet of old
and new bootloaders stays interoperable with the same Router.

**GCC 7 LTO must stay disabled for this build.** The startup object defines weak aliases for every
IRQ, and the production LTO link discarded the strong handlers from `stm32g0xx_it.c` in favour of
those aliases. `HAL_Init()` then enabled SysTick and the first tick entered `Default_Handler`,
where IWDG later reset the MCU. The non-LTO build resolves the vector table correctly and remains
comfortably inside the 24 kB bootloader bank.

**`FWUpdateApp::processIncoming` sized a stack VLA from an unbounded wire value.**
`packetBodyAndChecksumSize16` came straight off the wire as a `uint16_t` with no upper
bound before sizing `uint8_t dataWithChecksum[packetBodyAndChecksumSize]` — a corrupt or
malicious frame claiming up to 65,535 bytes would smash the stack of an image only
re-flashable via ST-Link. A value smaller than `sizeof(CRCType)` also underflowed
`packetBodySize` (unsigned wraparound). Now rejected before the VLA is declared:
`packetBodyAndChecksumSize16` must be in `[sizeof(CRCType), FW_FRAME_SIZE +
FW_CHECKSUM_SIZE]`. `test-native/fw_bounds_check_test.cpp` regression-tests this against
the real parser.

**`flash_write` read past the caller's buffer on a non-8-byte-multiple chunk.** The
double-word program loop advanced a raw `uint64_t*` from `src` to `src + size`
regardless of alignment, so any final chunk whose length wasn't a multiple of 8 read
past the buffer (the VLA above) for the last double-word and programmed whatever
was there. Now pads the final partial double-word with `0xFF` (flash's erased-state
value) instead.

**Safe IRQ handoff around the bootloader -> application jump.** `run_application()` used to swap
`SCB->VTOR` and reload MSP with no `__disable_irq()` guarding the transition, so a still-pending
NVIC interrupt could fire against the new vector table while still on the old stack. The first
attempt to fix that masked IRQs but did not restore PRIMASK before entering the application; on a
real board that trapped the application in its first interrupt-driven delay until IWDG reset it.
The handoff now leaves IRQs enabled through `HAL_RCC_DeInit()` (whose clock-transition waits use
the interrupt-driven HAL tick), then masks only while it disables and clears inherited NVIC state,
installs VTOR and MSP, and re-enables IRQs immediately before the application's reset handler.

**`SerialStream::getSerialStream` could fall off the end of a non-void function.** Same
category of bug as the `assert`/`lwrb_init` one above — undefined behaviour dressed up as
"this can't happen." Now returns `nullptr` explicitly in that case.

## Live-board verification

The corrected non-LTO bootloader and optical PortalFW application were flashed and verified on an
STM32G070RBT6 through Portal Test Bench. The post-flash probe check observed the application vector
table at `VTOR=0x08006000` and a stable running state; the former build instead trapped in the weak
SysTick alias at `Default_Handler` and was reset by IWDG.
The remaining bootloader-specific bench check is an actual RS485 field update through it — because
receiving firmware over RS485 is its primary job, and the one thing the SWD flash path does not
exercise.

**`cube-import/Core/msgpack-arduino` is still the 2023 snapshot**, not the submodule at
`PortalFW/lib/msgpack-arduino` that `test-native/` builds its 47 checks against. They have
diverged, and the shape of the divergence decides how much that matters:

| identical | diverged |
|---|---|
| `serialize`, `deserialize`, `Serializer`, `logError`, `NotArduino`, `Platform`, `constants.h` | `COBSRWStream` (89 lines), `DataType` (15), `Messaging` (13) |

The 47 checks are about frame-offset arithmetic, which runs entirely through `serialize` /
`deserialize` / `Serializer` — byte-identical in both copies. So they do prove what they claim
about both sides. That is luck rather than design, and worth re-checking if the tests ever grow.

The divergence itself is real and one line of it is load-bearing. The bootloader's
`COBSRWStream.hpp` hard-codes

```cpp
#define MSGPACK_COBSRWSTREAM_BUFFER_SIZE 64
```

where the submodule's says `256`, with a comment offering 64 as the smaller option. Fielded boards
therefore run a **64-byte COBS decode buffer in the bootloader and a 256-byte one in the
application**, and `protocol-hardening.md` §7.3's finding that `-D MSGPACK_COBSRWSTREAM_BUFFER_SIZE=64`
is inert is explained by this: the header defines it unconditionally, so the build flag never had
anything to do. The two copies also implement the buffer differently — a manual double-buffer with
`realignIncoming()` here, an `lwrb` ring buffer there.

Unifying them means changing what fielded bootloaders do to their receive path, which is not a
change to make alongside a build-system port.
