"""Post-build checks on the bootloader image.

This firmware can only be replaced with a debug probe, so a defect that reaches a board costs a
bench, a cable and physical access. These are the checks that can be made without either.

Five of them, in increasing order of how badly the thing they catch would go:

  1. Size against the 16 kB bank. The bank boundary is also where the application starts, so an
     image that overran it would be overwritten by the next application upload -- silently, and
     only on boards whose application happens to be large.

  2. The vector table's first two words. A wrong initial stack pointer or a reset vector outside
     the bank is a board that does not start, and neither is visible in a hex dump.

  3. The banner string. `portal-swd` identifies a bootloader by scraping "Bootloader v" out of a
     flash readback, because a string literal is the one piece of version information that needs
     no firmware cooperation to read. If it is optimised away, every host tool stops recognising
     the image.

  4. Symbols that must not be there. `printf` and friends arrive by accident -- one `sprintf` in
     an error path pulls in 1.9 kB of newlib -- and `malloc` in a bootloader with no heap is a
     failure waiting for the worst moment.

  5. RAM-resident code that branches into flash. The receive interrupt and the flash routines are
     copied into SRAM precisely so they keep working while flash is stalled by an erase. A call
     from one of them back into a flash-resident helper puts the stall back, and the symptom is
     not a crash: it is a bootloader that quietly stops hearing the bus for a second per page.
"""

Import("env")

import re
import subprocess

# The bank, from the same header the firmware, the host tools and the Rust crates all read.
LAYOUT = {}
_HEADER = env.subst("$PROJECT_DIR") + "/include/portal_flash_layout.h"
for _line in open(_HEADER):
    _match = re.match(r"^#define\s+(PORTAL_\w+)\s+(0x[0-9A-Fa-f]+)\s*(?:/\*.*)?$", _line)
    if _match:
        LAYOUT[_match.group(1)] = int(_match.group(2), 16)

BANK_BYTES = LAYOUT["PORTAL_BOOTLOADER_BYTES"]
FLASH_BASE = LAYOUT["PORTAL_FLASH_BASE"]
RAM_BASE = LAYOUT["PORTAL_RAM_BASE"]
RAM_END = LAYOUT["PORTAL_RAM_END"]
HANDOFF_ADDR = LAYOUT["PORTAL_HANDOFF_ADDR"]
FLASH_END = LAYOUT["PORTAL_FLASH_END"]

# Room left for the checks that cannot be made here: the ones that need a board.
RESERVE_BYTES = 512
HARD_LIMIT = BANK_BYTES - RESERVE_BYTES
# Where "comfortable" stops. Not an error, but the point at which the next feature needs a plan.
WARN_LIMIT = BANK_BYTES - 1536

BANNER_PREFIX = b"Bootloader v"

FORBIDDEN_SYMBOLS = (
    ("printf", "newlib formatting: ~1.9 kB, and every use of it in a bootloader has a smaller form"),
    ("sprintf", "as above, and the v4 bootloader's two calls also returned dangling stack pointers"),
    ("malloc", "this image has no heap; the linker script sets _Min_Heap_Size to zero"),
    ("_sbrk", "as above"),
)


def _tool(name):
    """The cross-tool beside the compiler this build is using."""
    cc = env.subst("$CC").strip()
    # `$CC` may be a bare name resolved through PATH, or a full path, and on some hosts carries
    # the toolchain version in the directory name. Splitting at the last dash of the *basename*
    # is what survives both.
    import os
    directory, base = os.path.split(cc)
    tool = base.rsplit("-", 1)[0] + "-" + name
    return os.path.join(directory, tool) if directory else tool


def _fail(message):
    print("\n*** bootloader size gate: %s\n" % message)
    env.Exit(1)


def _check_size(binary):
    with open(binary, "rb") as handle:
        image = handle.read()

    print("")
    print("  bootloader image  %6d bytes" % len(image))
    print("  bank              %6d bytes (%d reserved)" % (BANK_BYTES, RESERVE_BYTES))
    print("  headroom          %6d bytes" % (BANK_BYTES - len(image)))

    if len(image) > HARD_LIMIT:
        _fail(
            "%d bytes will not fit the %d-byte bank with %d reserved.\n"
            "    The bank boundary is where the application starts: an image past it is\n"
            "    overwritten by the next application upload."
            % (len(image), BANK_BYTES, RESERVE_BYTES)
        )
    if len(image) > WARN_LIMIT:
        print("  NOTE: past %d bytes there is no longer room for a feature without a plan."
              % WARN_LIMIT)
    return image


def _check_vectors(image):
    if len(image) < 8:
        _fail("image is too short to contain a vector table")
    stack_pointer = int.from_bytes(image[0:4], "little")
    reset_vector = int.from_bytes(image[4:8], "little")

    # The stack grows down, so the top of RAM is the correct value rather than an off-by-one.
    if stack_pointer != HANDOFF_ADDR:
        _fail(
            "initial stack pointer is 0x%08X, expected 0x%08X (the top of RAM below the\n"
            "    handoff block). The linker script's RAM length is what sets this."
            % (stack_pointer, HANDOFF_ADDR)
        )
    if not (RAM_BASE < stack_pointer <= RAM_END):
        _fail("initial stack pointer 0x%08X is outside SRAM" % stack_pointer)
    if reset_vector & 1 == 0:
        _fail("reset vector 0x%08X has no Thumb bit; it is not a Cortex-M entry point"
              % reset_vector)
    if not (FLASH_BASE <= (reset_vector & ~1) < FLASH_BASE + BANK_BYTES):
        _fail("reset vector 0x%08X is outside the bootloader bank" % reset_vector)

    print("  vectors           SP 0x%08X  reset 0x%08X" % (stack_pointer, reset_vector))


def _check_banner(image):
    if BANNER_PREFIX not in image:
        _fail(
            "the banner string %r is not in the image.\n"
            "    `portal-swd` identifies a bootloader by scraping it out of a flash readback,\n"
            "    so without it every host tool stops recognising this build."
            % BANNER_PREFIX.decode()
        )
    at = image.index(BANNER_PREFIX)
    end = image.index(b"\0", at)
    print("  banner            %s" % image[at:end].decode("ascii", "replace"))


def _symbols(elf):
    """(address, size, kind, section, name) for every symbol.

    Read in `sysv` format rather than the default. The default output identifies a symbol by a
    single letter, and which letter a *function placed in a data section* gets is not consistent
    between binutils builds -- the toolchain's own nm calls the RAM-resident handlers `t`, while
    the one PATH resolves calls them `d`. Since `.RamFunc` deliberately lives inside `.data`, a
    check keyed on that letter passes or fails depending on which nm ran. `sysv` states the type
    and the section outright.
    """
    try:
        output = subprocess.check_output(
            [_tool("nm"), "--format=sysv", "--print-size", elf],
            universal_newlines=True,
        )
    except (OSError, subprocess.CalledProcessError) as error:
        _fail("could not run %s: %s" % (_tool("nm"), error))
        return []

    found = []
    for line in output.splitlines():
        parts = [field.strip() for field in line.split("|")]
        if len(parts) < 7 or not parts[1]:
            continue
        try:
            address = int(parts[1], 16)
            size = int(parts[4], 16) if parts[4] else 0
        except ValueError:
            continue
        found.append((address, size, parts[3], parts[6].split()[0] if parts[6] else "", parts[0]))
    return found


def _check_forbidden(symbols):
    names = {name for _, _, _, _, name in symbols}
    for needle, why in FORBIDDEN_SYMBOLS:
        hits = sorted(name for name in names if needle in name)
        if hits:
            _fail("%s is linked in (%s).\n    %s" % (needle, ", ".join(hits), why))


def _check_ram_functions(elf, symbols):
    """No RAM-resident function may branch into flash.

    Disassembles every symbol that ended up in SRAM and looks for a branch to a 0x08xxxxxx target.
    The linker inserts veneers for long branches without complaint, so this is not something the
    build would otherwise report.
    """
    ram_functions = [
        (address, size, name)
        for address, size, kind, _section, name in symbols
        if kind == "FUNC" and RAM_BASE <= address < RAM_END and size > 0
    ]
    if not ram_functions:
        _fail(
            "no functions were placed in SRAM.\n"
            "    The receive interrupt and the flash routines must be, or reception stops for\n"
            "    the duration of every page erase. Check that .RamFunc is still inside .data."
        )

    # `-D`, not `-d`: `.RamFunc` lives inside `.data`, which is not flagged as code, so a
    # plain disassembly would skip exactly the functions being checked.
    disassembly = subprocess.check_output(
        [_tool("objdump"), "-D", elf], universal_newlines=True
    )
    current = None
    offenders = []
    for line in disassembly.splitlines():
        header = re.match(r"^([0-9a-f]+) <(.+)>:$", line)
        if header:
            current = (int(header.group(1), 16), header.group(2))
            continue
        if current is None or not (RAM_BASE <= current[0] < RAM_END):
            continue
        branch = re.search(r"\b(b|bl|b\.w|bl\.w|blx)\s+([0-9a-f]+)\s+<", line)
        if branch:
            # Into *flash*, specifically. RAM addresses are numerically higher than FLASH_BASE, so
            # a naive lower bound would flag every call one RAM function makes to another.
            target = int(branch.group(2), 16)
            if FLASH_BASE <= target < FLASH_END:
                offenders.append("%s -> 0x%08X" % (current[1], target))

    if offenders:
        _fail(
            "RAM-resident code branches into flash:\n      %s\n"
            "    These routines run while flash is stalled by an erase; a call back into flash\n"
            "    stalls with it, and the bootloader stops hearing the bus mid-update."
            % "\n      ".join(sorted(set(offenders)))
        )

    print("  in SRAM           %s" % ", ".join(sorted(name for _, _, name in ram_functions)))


def _print_largest(symbols):
    code = [entry for entry in symbols if entry[2] == "FUNC"]
    biggest = sorted(code, key=lambda entry: entry[1], reverse=True)[:15]
    print("\n  largest functions")
    for _, size, _, _, name in biggest:
        print("    %6d  %s" % (size, name))
    print("")


def gate(source, target, env):
    binary = str(target[0])
    elf = binary[: -len(".bin")] + ".elf"

    image = _check_size(binary)
    _check_vectors(image)
    _check_banner(image)

    symbols = _symbols(elf)
    _check_forbidden(symbols)
    _check_ram_functions(elf, symbols)
    _print_largest(symbols)


env.AddPostAction("$BUILD_DIR/${PROGNAME}.bin", gate)
