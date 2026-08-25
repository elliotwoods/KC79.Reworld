# PortalBootloader

The RS485 field-update bootloader for KC79 Portal boards. 16 kB at `0x08000000`; the application
sits above it at `0x08004000`.

```sh
pio run -e bootloader     # the image that goes on a board
pio test -e native        # the protocol, on a laptop, in three seconds
```

## Why this was rewritten

The bootloader burned into every fielded board erases three pages more than the application bank.
Those three pages hold the board's provisioning serial number and both settings journals, so **every
field update destroyed a board's identity**, and nothing said so — the update itself succeeded, and
the loss only surfaced later as a board that could no longer be told apart from any other.

That is the defect this replaces. Since a bootloader can only be replaced rarely and (until now)
only with a debug probe, everything else worth fixing was fixed at the same time:

| | fielded v4/v5 | v6 |
|---|---|---|
| erase | 52 pages — three past the application bank | 53 pages, bounded at `0x0801E800` |
| durable pages | destroyed on every update | never written, never erased |
| addressing | broadcast only; no address of its own | addressed, and answerable by serial or MCU UID |
| replies | none — it never transmits | every request answered, with a sequence number |
| lost frame | ends the upload silently, host reports success | reported by a received-chunk bitmap and repaired |
| frame order | strictly increasing offsets only | any order; duplicates are free |
| integrity | 16-bit XOR over the payload | that, plus a CRC-16 over the whole frame, plus a CRC-32C over the programmed image |
| reception during erase | deaf for over a second | continuous — the receive path runs from SRAM |
| residency | 3 s, fixed | 3 s, or 30 s when the application asks for it, or indefinite mid-session |
| size | 22,708 bytes of 24 kB | 14,796 bytes of 16 kB |

The image is 8 kB smaller, which is where the application's extra 8 kB comes from.

## Layout

```
platformio.ini            two environments: `bootloader` (STM32) and `native` (host tests)
bootloader.ld             16 kB bank, RAM short of the handoff block, .ram_vectors, .RamFunc
include/
  portal_flash_layout.h   THE flash map. Shared with PortalFW, portal-swd and tools/. See below.
  portal_crc32c.h         the checksum every durable structure on the board is checked with
  bl/*.hpp                the bootloader's own headers
src/
  core/                   hardware-independent: parser, session, run decision, state machine
  target/                 STM32 only: clock, vectors, ISRs, flash, UART, the hardware seam
  stm32g0xx_hal_conf.h    enables no HAL module at all — see the comment in it
lib/bltest/               host fakes: a flash that refuses to reprogram, a settable clock
test/                     83 checks over everything in src/core
tools/size_gate.py        five post-build checks, described below
reference/                the 2023 image every un-updated board still runs
```

### `include/portal_flash_layout.h` is the flash map, for everyone

The same numbers used to be written out four times — here, in `PortalFW/set_bank2.py`, in
`portal-swd`'s `addr` module, and in `tools/firmware.mjs` — and nothing checked that they still
agreed. They are load-bearing in the way that a wrong one destroys a board's provisioning rather
than failing a build.

So there is one definition, and because three of its four readers are not C compilers, it is also
parsed as text. That is why every value in it is a bare hexadecimal literal with no arithmetic.
Four things read it and fail if they disagree:

- `router_proto::layout` and `portal_swd::addr` (Rust, `include_str!` plus a test per constant)
- `tools/firmware.mjs` (regex)
- `PortalFW/layout_check.py`, which asserts the build's own bounds against it before linking

## How an update works

Two protocols share the wire. A v6 bootloader speaks both, which is what lets a fleet be updated
one board at a time rather than all at once.

**The legacy flow** is what an un-updated Router sends and is unchanged: broadcast `"FW"` announces,
`"ER"`, `{offset: bin(xor16 ++ data)}` frames, `"RU"`. A v6 bootloader accepts all of it — in any
frame order now, and writing at the **legacy** base, because a host old enough to send `"ER"` is
sending an image linked for `0x08006000`.

**The addressed flow** is `{"bl": {...}}` bodies with a sequence number and CRC-16 trailer:

| verb | what it is for |
|---|---|
| `status` | version, address and where it came from, serial, UID, bank geometry, state, and the installed application's own version |
| `begin` | erase the bank and declare length, CRC-32C and chunk size. **Answered when the erase finishes**, about 1.2 s later, so the host waits for a fact rather than a guess |
| `map` | which chunks arrived, as a bitmap |
| `verify` | CRC-32C over what was actually programmed |
| `run` | start the application, or say why not |
| `adopt` | take an RS485 address |
| `reset` | reply, then reset |

A unicast request is answered by its target. A broadcast is answered by **nobody** unless it carries
a selector naming one board by serial or UID — six boards answering one frame on a half-duplex bus
is a collision, not a conversation. A broadcast without a selector still *acts*, which is how one
`begin` opens a session on fifty-four boards at once.

### Where the address comes from

A bootloader has no address of its own: the RS485 id is assigned by a daisy-chain the *application*
runs. So before the application resets into the bootloader it writes a 32-byte block at the top of
SRAM — id, serial, and whether an update is expected — which startup neither copies nor zeroes.
Failing that, `adopt`, and failing that the four ID switches, exactly as the application maps them.
`status` reports which of the three it used, because a switch-derived address is a fallback several
boards on a branch may share and a host should prefer a serial selector when it sees one.

### Refusing to start the wrong image

An application linked for `0x08004000` and one linked for `0x08006000` are indistinguishable by
inspection — the banks overlap, so even the reset vector cannot separate them — and starting the
wrong one hard-faults later at an unrelated address. So an image states its own base in a descriptor
at `base + 0xC0`, and the bootloader refuses anything at the new base that does not say, or that
says the wrong thing.

The legacy base is tried only when the new bank is **entirely blank**, which is exactly the state a
board is in between having its bootloader replaced and having its application re-uploaded. Without
that fallback, updating a fleet would be a flag day.

## Two things that are worth knowing before changing this

**The receive path runs from SRAM, and that is not optional.** Erasing a page stalls every flash
read for 20-25 ms, so code fetched from flash cannot execute during it — including the UART receive
interrupt. That is why the old bootloader was deaf for over a second after `"ER"`, and why the host
had to blanket each erase in three seconds of announce frames and send it twice. The interrupt, the
vector table and the flash routines are therefore all copied into SRAM, and `tools/size_gate.py`
disassembles the built image to check that none of them branches back into flash. A single
accidental call would restore the deafness silently.

**`COBSRWStream::available()` merges packets if called twice before the first `read()`.** It only
stops at a packet boundary once the reader has consumed a byte, so a second call decodes straight
through the delimiter and appends the next packet to the current one; the merged tail is then
discarded when the reader advances, and the second frame vanishes without a trace. Every
`waitForData` inside every parse calls `available()`, so this is reachable from ordinary parsing,
and two frames arriving back to back is the *normal* case during an upload. `bl::FrameWindow` exists
for this: it buffers exactly one frame and shows the codec nothing past its delimiter. This was
found by a test, not by reading.

## The size gate

`pio run -e bootloader` fails rather than warns on any of:

1. the image not fitting the 16 kB bank with 512 bytes reserved;
2. an initial stack pointer that is not the top of RAM below the handoff block, or a reset vector
   outside the bank or without its Thumb bit;
3. the `Bootloader v` banner missing — `portal-swd` identifies a board by scraping it out of a flash
   readback, so without it every host tool stops recognising the image;
4. `printf`, `sprintf`, `malloc` or `_sbrk` being linked in (one `sprintf` in an error path costs
   1.9 kB, and the v4 bootloader's two also returned pointers to stack that had gone out of scope);
5. RAM-resident code branching into flash.

## The tests

`pio test -e native` builds `src/core` for the host against `lib/bltest`'s fakes and the real
msgpack sources, and runs 83 checks in about three seconds. They are the ones that would otherwise
need a board, a probe and a bus.

The fake flash reproduces the two behaviours real flash has that a naive fake would not: an erased
double-word can be programmed once, and **programming a written one fails even when the value is
identical**. That second rule is why the session tracks written granules at all — duplicate frames
are routine, the legacy host sends every frame twice by default, and a fake that quietly accepted
repeat programming would let every test pass against firmware that bricks on the second frame of
every upload.

Two defects were found this way rather than on a bench: the packet-merging above, and a corrupted
array header being able to downgrade a trailered frame into an unverified one (a 3-element frame is
now required to end exactly where it says it does).

## Building

`platform = ststm32@19.6.0` is pinned, and has to be — unpinned, PlatformIO resolves whatever
release a machine already carries and the compiler comes with it, so two machines produce two
different images from one commit. 19.6.0 resolves GCC 7.2.1.

`-fno-lto` is not optional either: GCC 7's LTO resolves the startup object's weak IRQ aliases in
preference to the strong handlers, so the vector table ends up pointing every interrupt at
`Default_Handler`. The image verifies byte-for-byte and traps on its first SysTick.

`board_build.stm32cube.custom_config_header = yes` is required and silent when missing. Without it
the framework compiles against its own `stm32g0xx_hal_conf.h` and this project's — which enables no
HAL module at all — is ignored, and about 6 kB of HAL arrives that nothing calls.
