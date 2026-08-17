# The STM32CubeIDE project, imported unmodified

This is the RS485 field-update bootloader as it exists in STM32CubeIDE, copied here **without a
single byte changed**, so that the PlatformIO port lands as a reviewable diff rather than as a
directory that appeared fully formed.

It does not build. That is the next commit's job.

## Where it came from

```
C:\Users\user\Kimchi and Chips Dropbox\Elliot Woods\KC79 - SBAU\Engineering\
    STM32CubeWorkspace\BootloaderRS485
```

A Dropbox path outside any repository, which is precisely why it is here now: the only copy of
this firmware's source lived somewhere with no history, no review and no backup that a `git log`
can see.

The `Release/` build in that tree produced the binary already committed at
`../reference/BootloaderRS485-2023-08-26.bin` — same 22,708 bytes, same SHA-256
`6a809709…f833e21`. So the sources here and the reference image are a matched pair, which is what
makes the structural diff in the third commit meaningful rather than approximate.

## What was copied, and what was not

| | |
|---|---|
| `Core/` | Copied. 47 files: the application, the msgpack vendored copy, `syscalls`/`sysmem`, the CMSIS system file, and `Startup/startup_stm32g070rbtx.s`. |
| `STM32G070RBTX_FLASH.ld` | Copied. **Says `LENGTH = 28K`** for what is a 24 kB bank — see `protocol-hardening.md` §7.3. |
| `BootloaderRS485.ioc` | Copied. The CubeMX configuration, which is the record of *why* the peripheral setup is what it is. |
| `.cproject` `.project` `.mxproject` | Copied. Not useful to PlatformIO, but they are what documents the original include paths and defines, and guessing at those is how a port silently changes behaviour. |
| `Drivers/` | **Not copied.** 7.3 MB of ST's HAL and CMSIS, which is exactly what PlatformIO's `framework-stm32cube` supplies. Vendoring a second copy to delete it one commit later would put 7.3 MB in this repository's history permanently. |
| `Debug/` `Release/` | **Not copied.** 36 MB of object files and build output. The one artefact worth keeping — the linked image — is already in `../reference/`, now with its `.elf` beside it. |

"Unmodified" means no source file was edited. Two directories of build output and one of a vendored
dependency are not source.

## `Core/msgpack-arduino` is a copy, not the submodule

The repository already carries `PortalFW/lib/msgpack-arduino` as a git submodule, and
`../test-native/` builds its 47 checks against **those** sources. This directory holds a 2023
snapshot of the same library.

Whether they have diverged is a question for the porting commit, not this one, and it is a real
question: `PortalBootloader/test-native/README.md` records that the frame-offset arithmetic was
verified against the submodule, so a divergence here would mean those tests proved something about
code this bootloader does not actually run.

## Known before starting

From `protocol-hardening.md` §7.2 as corrected by §7.3, so none of these is discovered twice:

- The linker script says `LENGTH = 28K` where the bank is 24 kB.
- `-D MSGPACK_COBSRWSTREAM_BUFFER_SIZE=64` is inert — the header does not consult it.
- `board_build.stm32cube.custom_config_header = yes` is required, or `stm32g0xx_hal_conf.h` here is
  ignored in favour of the framework's.
- Bit-for-bit reproduction is unattainable: this was built with GCC 10 and PlatformIO ships 12. The
  gate is **behavioural equivalence, structurally diffed**, against
  `../reference/BootloaderRS485-2023-08-26.elf` — 370 symbols, `text 22692 · data 16 · bss 2884`.
- The budget is **1,868 bytes** of headroom in the 24 kB bank. If GCC 12 overshoots it, the largest
  single lever is the two `sprintf` calls in `Core/Src/flash.cpp` (lines 39 and 78), whose removal
  also fixes a use-after-return.
