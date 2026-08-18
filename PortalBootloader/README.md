# PortalBootloader

The RS485 field-update bootloader for KC79 Portal boards. 24 kB at `0x08000000`; the application
sits above it at `0x08006000`.

```powershell
pio run -e bootloader          # what goes on a board
pio run -e bootloader_nolto    # the same code, comparable
```

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

| | reference | `bootloader` | `bootloader_nolto` |
|---|---|---|---|
| text | 22,692 | 13,988 | 19,396 |
| data | 16 | 128 | 12 |
| bss | 2,884 | 2,884 | 2,880 |
| defined symbols | 370 | 219 | 323 |
| initial SP | `0x20009000` | `0x20009000` | `0x20009000` |
| reset vector | `0x0800123D` | `0x08002A79` | `0x08003ECD` |
| banner | `Bootloader v4` | `Bootloader v5` | `Bootloader v5` |

(The `text`/reset-vector numbers above are after the fixes in the next section — the defined-symbol
count is unchanged by them, since every fix edits an existing function rather than adding one.)

Two effects account for the whole of the difference, and neither is missing functionality.

**LTO.** The shipping build folds nearly everything into `main`, which ends up 3,216 bytes long.
`HAL_DMA_Init`, `flash_erase`, `FWUpdateApp::processIncoming` and sixty-odd others are present as
inlined code with no symbol left on them. That is why `bootloader_nolto` exists: under `-fno-lto`
every function keeps its name, so a symbol absent from that list is genuinely absent from the
firmware.

**newlib.** Of the 56 symbols in the reference and not in `bootloader_nolto`, **every one** is
newlib internals — `__sfp`, `__swsetup_r`, `_fflush_r`, `fiprintf`, `sbrk_aligned` and their
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

**Clock raised to match the application (`PLLN` 8 -> 16, 32 -> 64 MHz).** Both the
bootloader and the application already run off the same 8 MHz HSE crystal through the
PLL; the bootloader was just at half the application's multiplier. UART baud is
recomputed from the live peripheral clock at `MX_USART{1,2}_UART_Init` time (which runs
after `SystemClock_Config`), and `HAL_RCC_ClockConfig()` re-derives SysTick internally on
every call, so this is a pure internal timing change. `FLASH_LATENCY_1` -> `_2`, required
above 48 MHz at voltage scale 1. Motivation: today's RS485 uploads need to be slowed down
and still see occasional failures — this is the most direct lever available without
touching the protocol, and needs a bench pass to confirm the real electrical margin.

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

**No IRQ masking around the bootloader -> application jump.** `run_application()` swaps
`SCB->VTOR` and reloads MSP with no `__disable_irq()` guarding the transition — a still-
pending NVIC interrupt could in principle fire against the *new* vector table while still
on the *old* stack pointer. `__disable_irq()` now runs immediately before
`HAL_RCC_DeInit()`, matching normal jump-to-application practice.

**`SerialStream::getSerialStream` could fall off the end of a non-void function.** Same
category of bug as the `assert`/`lwrb_init` one above — undefined behaviour dressed up as
"this can't happen." Now returns `nullptr` explicitly in that case.

## Not yet done

**It has not been run on a board.** Everything above is static comparison. The bench check is
flashing it with `PortalFlasher`, confirming the banner reads back, and then performing an actual
RS485 field update through it — because receiving firmware over RS485 is the entire job, and the
one thing no amount of symbol diffing can demonstrate.

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
