// Deciding whether there is an application worth starting, and where.
//
// # Why this is more than "jump to 0x08004000"
//
// A v6 bootloader has to serve two populations at once. Boards updated in place still carry an
// application linked for the old `0x08006000`; boards flashed since carry one linked for
// `0x08004000`. Both are correct, and the bootloader cannot know which it has without looking.
//
// Looking is harder than it sounds, because the two images are indistinguishable by inspection.
// The banks overlap: an image at `0x08004000` has a reset vector inside `[0x08006000, 0x0801E800)`
// just as often as one at `0x08006000` does, and both have a stack pointer in SRAM. Starting the
// wrong one does not fail at the jump -- it fails later, at the first absolute address the code
// dereferences, as a hard fault with no relationship to the actual mistake.
//
// So an image says which bank it was built for, in a descriptor at a fixed offset, and the
// bootloader refuses anything that does not say. The one exception is the legacy base, where no
// descriptor can exist because every image that runs there predates the idea -- and that exception
// is safe only because it is tried *last*, and only when the new bank is entirely blank.
#pragma once

#include <stdint.h>

#include "bl/config.hpp"
#include "bl/errors.hpp"

namespace bl {

	struct RunDecision {
		bool ok = false;
		uint32_t base = 0;
		Error error = Error::NoApp;
	};

	/// Whether the vector table at `base` could plausibly start.
	///
	/// Deliberately weak: an initial stack pointer inside SRAM and a Thumb reset vector inside the
	/// application region. It cannot tell a good image from a corrupt one -- that is what the
	/// descriptor and the session CRC are for -- only a programmed image from an erased or absent
	/// one.
	bool vectorTableValid(uint32_t base);

	/// The application descriptor at `base + 0xC0`, or null if there is none.
	const portal_app_descriptor_t * descriptorAt(uint32_t base);

	/// Whether every byte of `[address, address + length)` reads as erased.
	bool regionBlank(uint32_t address, uint32_t length);

	/// Which application to start, if any.
	///
	/// `sessionCrc`, when non-zero, is the CRC-32C an open session declared for the image it just
	/// received: a mismatch means the upload did not complete, and is refused before anything else
	/// is considered. Pass 0 when no session is open.
	RunDecision decideRun(uint32_t sessionLength, uint32_t sessionCrc, uint32_t sessionBase);

	/// CRC-32C over programmed flash, for `verify`.
	///
	/// Kicks the watchdog as it goes: a full 108 kB bank takes roughly 70 ms at 64 MHz, which is
	// well inside the ~4.1 s watchdog period, but the same routine is called from contexts where
	/// other work has already used part of it.
	uint32_t crcOverFlash(uint32_t base, uint32_t length);

} // namespace bl
