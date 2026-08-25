"""Check this build's flash and RAM bounds against the shared layout header, before it links.

Replaces `set_bank2.py`, which set two things and got one of them wrong in a way nothing noticed.

`set_bank2.py` injected `-Wl,--defsym=LD_FLASH_SIZE=0x18800` to bound the application below the
three durable pages. The variant linker script does not reference `LD_FLASH_SIZE` -- it uses
`LD_MAX_SIZE`, which the framework supplies unconditionally from `upload.maximum_size`, the whole
128 kB part. So the defsym did nothing, the application linked with a FLASH window running to
0x08020000, and an image that grew past 98 kB would have been placed on top of the board's
provisioning identity. Nothing in the build would have said so; the first symptom would have been
a board losing its serial number the next time its settings were written.

So this script asserts, rather than sets: every number the build is about to use is compared with
`PortalBootloader/include/portal_flash_layout.h`, the same file the bootloader compiles against and
the Rust and JavaScript tooling parse. A disagreement stops the build with the two values named.

It also injects `LD_APP_DESCRIPTOR_OFFSET`, which the linker script needs and which is not a
number this project should be able to write down twice.
"""

Import("env")

import os
import re

BOARD = env.BoardConfig()

HEADER = os.path.join(
    env.subst("$PROJECT_DIR"), "..", "PortalBootloader", "include", "portal_flash_layout.h"
)


def _layout():
    """The shared header, as a dict of name -> integer."""
    if not os.path.exists(HEADER):
        _fail(
            "the shared layout header is missing:\n      %s\n"
            "    PortalFW and PortalBootloader are built from the same flash map; without it this\n"
            "    build cannot know where the application bank ends." % HEADER
        )
    values = {}
    with open(HEADER) as handle:
        for line in handle:
            match = re.match(r"^#define\s+(PORTAL_\w+)\s+(0x[0-9A-Fa-f]+)\s*(?:/\*.*)?$", line)
            if match:
                values[match.group(1)] = int(match.group(2), 16)
    return values


def _fail(message):
    print("\n*** PortalFW layout check: %s\n" % message)
    env.Exit(1)


def _expect(what, actual, expected):
    if actual != expected:
        _fail(
            "%s is 0x%X, expected 0x%X.\n"
            "    platformio.ini and %s disagree; fix whichever is wrong rather than\n"
            "    loosening this check." % (what, actual, expected, os.path.basename(HEADER))
        )


layout = _layout()

FLASH_BASE = layout["PORTAL_FLASH_BASE"]
APP_BASE = layout["PORTAL_APP_BASE"]
APP_BASE_LEGACY = layout["PORTAL_APP_BASE_LEGACY"]
APP_END = layout["PORTAL_APP_END"]
RAM_BASE = layout["PORTAL_RAM_BASE"]
HANDOFF_ADDR = layout["PORTAL_HANDOFF_ADDR"]
DESCRIPTOR_OFFSET = layout["PORTAL_APP_DESCRIPTOR_OFFSET"]

flash_offset = int(str(BOARD.get("build.flash_offset", "0x0")), 16)
base = FLASH_BASE + flash_offset

# Which bank is this build for? Only two answers are ever correct, and both are current: boards on
# bootloader v4/v5 run at the legacy base, boards on v6 at the new one.
if base not in (APP_BASE, APP_BASE_LEGACY):
    _fail(
        "board_build.flash_offset 0x%X puts this image at 0x%08X, which is neither application\n"
        "    base (0x%08X for bootloader v6, 0x%08X for v4/v5)."
        % (flash_offset, base, APP_BASE, APP_BASE_LEGACY)
    )

# The flash window must stop at the first durable page, not at the end of the part.
_expect("board_upload.maximum_size", int(BOARD.get("upload.maximum_size")), APP_END - FLASH_BASE)

# And RAM must stop below the handoff block, or the stack grows into the note this image leaves
# for the bootloader.
_expect(
    "board_upload.maximum_ram_size",
    int(BOARD.get("upload.maximum_ram_size")),
    HANDOFF_ADDR - RAM_BASE,
)

# The upload address has to be the same number the image was linked for, or it programs cleanly
# somewhere it cannot run.
upload_offset = BOARD.get("upload.offset_address", None)
if upload_offset is not None:
    _expect("board_upload.offset_address", int(str(upload_offset), 16), base)

env.Append(
    LINKFLAGS=["-Wl,--defsym=LD_APP_DESCRIPTOR_OFFSET=%s" % hex(DESCRIPTOR_OFFSET)]
)

print(
    "PortalFW layout: application 0x%08X-0x%08X (%d bytes), RAM to 0x%08X, descriptor at +0x%X"
    % (base, APP_END, APP_END - base, HANDOFF_ADDR, DESCRIPTOR_OFFSET)
)
