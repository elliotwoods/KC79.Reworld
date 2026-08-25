// Reading the provisioning identity out of the durable pages.
//
// The bootloader needs the serial number for one reason: it is how a host addresses a board whose
// RS485 id it does not know. That is not a rare case -- a board that power-cycled has no
// application to tell the bootloader its id, and the DIP fallback is only unique within a branch
// -- so without it a board that failed an update could not be singled out to retry.
//
// This is a *reader only*. The bootloader never writes these pages, never erases them, and its
// erase loop is bounded so it cannot reach them by accident. The v4 bootloader's erase ran to the
// end of flash and destroyed them on every update, which is the defect this whole rewrite exists
// to fix; the reading code sitting here is a good place to say so.
//
// The record format is defined by `PortalFW/src/PersistentStorage.cpp` and mirrored in
// `PortalFlasher/crates/portal-swd/src/persistent.rs`, which carries the golden byte vectors.
#pragma once

#include <stdint.h>

#include "bl/config.hpp"

namespace bl {

	struct Identity {
		bool valid = false;
		/// A record was found but belongs to a different MCU, so it is not ours to answer for.
		/// Distinguished from "absent" because it means a page was cloned between boards, which is
		/// a provisioning mistake worth being able to see.
		bool foreignUid = false;
		uint32_t serial = 0;
		uint32_t generation = 0;
	};

	/// Scan the identity page and return the highest-generation valid record for this MCU.
	Identity readIdentity();

	/// The same scan against an explicit page image, for tests.
	Identity scanIdentityPage(const uint8_t * page, const uint32_t ownUid[3]);

} // namespace bl
