// The image's own account of which flash bank it was built for.
//
// # Why an image has to say
//
// An application linked for 0x08004000 and one linked for 0x08006000 are indistinguishable by
// inspection. Both have an initial stack pointer inside SRAM and a reset vector with the Thumb bit
// set somewhere inside the application region -- the two banks overlap, so even the reset vector
// cannot separate them. Starting the wrong one does not fail at the jump: it fails later, at the
// first absolute address the code dereferences, as a hard fault with no relationship to the
// mistake. On a board that can only be recovered with a debug probe, that is an expensive way to
// find out.
//
// Both builds sit side by side in `.pio/build/` with names differing by a suffix, and both are
// current: boards still carrying the old bootloader run at the legacy base, boards carrying v6 run
// at the new one. So the image states its base, the bootloader refuses to start one whose
// statement disagrees with where it is sitting, and the host tooling refuses to send one to a
// board that would not run it.
//
// # Why the placement is exact
//
// `ldscript_app.ld` puts this at exactly `ORIGIN(FLASH) + 0xC0`, and asserts the vector table has
// not grown into it. The bootloader reads it out of an image it did not build, so "wherever the
// linker felt like putting it after the vectors" is not a contract -- orphan-section placement
// depends on link order, and would change silently.

#include "portal_flash_layout.h"
#include "Version.h"

// `used` because nothing references it: the only reader is a separate image, reached through a
// fixed address, so every ordinary reachability rule would discard this.
extern "C" __attribute__((section(".app_descriptor"), used, aligned(4)))
const portal_app_descriptor_t g_app_descriptor = {
	// Taken character by character from the shared macro. The field is exactly eight bytes and
	// carries no terminator, which C++ will not initialise from a nine-byte string literal -- but
	// indexing the literal keeps this derived from the one definition rather than a second copy
	// of it that could drift.
	{
		PORTAL_APP_DESCRIPTOR_MAGIC[0], PORTAL_APP_DESCRIPTOR_MAGIC[1],
		PORTAL_APP_DESCRIPTOR_MAGIC[2], PORTAL_APP_DESCRIPTOR_MAGIC[3],
		PORTAL_APP_DESCRIPTOR_MAGIC[4], PORTAL_APP_DESCRIPTOR_MAGIC[5],
		PORTAL_APP_DESCRIPTOR_MAGIC[6], PORTAL_APP_DESCRIPTOR_MAGIC[7],
	},
	// Where this image was linked. `VECT_TAB_OFFSET` is injected by the build from
	// `board_build.flash_offset`, which is the same number `board_upload.offset_address` uses --
	// so the descriptor cannot disagree with where the image is actually flashed.
	PORTAL_FLASH_BASE + VECT_TAB_OFFSET,
	0,
	PORTAL_VERSION_STRING,
};

static_assert(sizeof(PORTAL_VERSION_STRING) <= PORTAL_APP_VERSION_BYTES + 1,
	"PORTAL_VERSION_STRING does not fit the descriptor's version field");

// If the magic ever grows or shrinks, the initialiser above stops being right and this is what
// says so.
static_assert(sizeof(PORTAL_APP_DESCRIPTOR_MAGIC) == sizeof(g_app_descriptor.magic) + 1,
	"the descriptor magic is eight bytes with no terminator");
