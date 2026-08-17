# Reference bootloader

`BootloaderRS485-2023-08-26.bin` — the bootloader that is on fielded Portal boards, kept here so
a full device image can be flashed before the PlatformIO port exists, and so that port has a
byte-exact target to be compared against.

**This is a binary whose source is not in this repository.** That is why it is quarantined in its
own directory with its provenance written down, rather than dropped somewhere convenient. Once
`PortalBootloader` builds from source and passes its bench tests, the flasher prefers the built
one and this becomes a comparison artefact.

## Provenance

Copied unmodified from the STM32CubeIDE project on Dropbox:

```
KC79 - SBAU/Engineering/STM32CubeWorkspace/BootloaderRS485/Release/BootloaderRS485.bin
```

Built 2023-08-26 14:58, GCC 10.3-2021.10 (the toolchain STM32CubeIDE shipped), `-Os`, with
`-flto`. The matching `.elf`, `.map` and `.list` are still in that project and are what §6 of the
import plan diffs against — they are not copied here, because only the image is needed to flash a
board.

| | |
|---|---|
| Size | 22,708 bytes |
| SHA-256 | `6a809709d0016e38bed5b79c6f6f8d37325750f67720bce5368881cb6f833e21` |
| Load address | `0x08000000` |
| Initial SP | `0x20009000` |
| Reset vector | `0x0800123D` |
| Banner | `Bootloader v4` |

## Why those four facts

They are what make it a *usable* image rather than merely a file of the right length, and each was
checked rather than assumed:

- **22,708 bytes** fits the 24 kB (24,576-byte) bootloader bank with 1,868 bytes spare. Anything
  larger overlaps the application at `0x08006000`. Note the CubeIDE linker script says
  `FLASH LENGTH = 28K`, so an oversized build would link cleanly and overlap silently — see
  `protocol-hardening.md` §7.3.
- **SP `0x20009000`** is the top of the 36 kB SRAM. The stack grows down, so pointing one past the
  last byte is correct.
- **Reset vector `0x0800123D`** is inside the bootloader bank with the Thumb bit set — so this is
  linked at `0x08000000`, not an application image mislabelled.
- **`Bootloader v4`** is a plain string literal in the binary, which is how the flasher identifies
  what is on a board without symbols or a firmware change.

`PortalFlasher` re-checks the size, the vector table and the hash when it loads this as an image;
nothing here is trusted because it is written down.

## Do not edit

If a new bootloader is built, add it beside this one with its own date and provenance rather than
replacing this file. Boards in the field carry *this* image, and being able to reproduce exactly
what they have is the reason it is here.
