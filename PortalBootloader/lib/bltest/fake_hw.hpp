// A fake STM32G070 for the native tests: 128 kB of flash, a clock you set, and a record of every
// terminal action instead of an execution that never returns.
//
// The point is not that it is convenient. It is that the fake reproduces the two behaviours of
// real flash that the bootloader's correctness actually depends on, and that a naive fake would
// get wrong:
//
//   - an erased double-word reads as 0xFF and can be programmed once;
//   - programming an already-programmed double-word **fails**, even when the value is identical.
//
// The second is what makes duplicate frames a real problem rather than a theoretical one, and it
// is the reason the session tracks written granules at all. A fake that quietly accepted repeat
// programming would let every one of those tests pass against a bootloader that bricks on the
// first duplicate frame the host sends -- which, with the legacy profile's `frame_repetitions: 2`,
// is every second frame.
#pragma once

#include <stdint.h>
#include <stddef.h>

#include "bl/hw.hpp"

namespace bltest {

	/// Reset every fake: flash to erased, clock to zero, records cleared.
	void reset();

	// ---- Flash -------------------------------------------------------------------------------

	/// The whole 128 kB, indexable from 0.
	uint8_t * flash();
	/// A pointer to an absolute address, for setting up a test's initial state.
	uint8_t * flashAt(uint32_t address);
	/// Fill a region as if it had been erased.
	void eraseRegion(uint32_t address, uint32_t length);
	/// Write bytes directly, bypassing the program rules, to set up a test.
	void preload(uint32_t address, const uint8_t * data, uint32_t length);
	/// Lay down a plausible vector table, and optionally a descriptor, at `base`.
	void preloadApplication(uint32_t base, uint32_t entryOffset, bool descriptor,
		uint32_t descriptorBase, const char * version);

	/// How many pages have been erased, and how many double-words programmed.
	uint32_t erasedPages();
	uint32_t programmedWords();
	/// Pretend the UART receive ring overran, to exercise the reporting path.
	void setRingOverran();

	/// Make the next erase of `page` fail, to exercise the flash-fault path.
	void failEraseOf(uint32_t page);
	/// Make the next program at `address` fail.
	void failProgramAt(uint32_t address);

	// ---- Identity ------------------------------------------------------------------------------

	void setUid(uint32_t a, uint32_t b, uint32_t c);
	void setDip(uint8_t value);

	// ---- Time -----------------------------------------------------------------------------------

	void setMillis(uint32_t value);
	void advance(uint32_t by);
	uint32_t watchdogKicks();

	// ---- Terminal actions -------------------------------------------------------------------------

	/// What the bootloader asked to happen at the end of its life.
	struct Terminal {
		bool reset = false;
		bool ran = false;
		uint32_t base = 0;
		uint32_t drains = 0;
	};

	const Terminal & terminal();

	// ---- Log and LEDs ----------------------------------------------------------------------------

	/// Everything written to the debug UART, as one string.
	const char * log();
	void clearLog();
	uint32_t ledToggles(bl::hw::Led led);

} // namespace bltest
