#include "Handoff.h"

#include "portal_crc32c.h"

#include <Arduino.h>
#include <string.h>

namespace {
	portal_handoff_t * block()
	{
		// A fixed address rather than a linker-placed object. The application and the bootloader
		// are separately linked images that have to agree on where this is, and an address both
		// read from the same header is a stronger agreement than two `.handoff` sections that
		// happen to land in the same place.
		return (portal_handoff_t *) PORTAL_HANDOFF_ADDR;
	}
}

namespace Handoff {

	//----------
	void write(int8_t id, uint32_t serial, uint8_t request)
	{
		portal_handoff_t * target = block();

		memset(target, 0, sizeof(*target));
		target->magic = PORTAL_HANDOFF_MAGIC;
		target->version = PORTAL_HANDOFF_VERSION;
		target->request = request;
		target->id = id;
		// Serial 0 is what a board with no provisioning identity reports, and it is never a valid
		// serial -- so the flag says "this field means something" rather than the value having to.
		target->flags = (serial != 0) ? PORTAL_HANDOFF_FLAG_SERIAL_VALID : 0;
		target->serial = serial;
		target->crc32c = portal_crc32c((const uint8_t *) target,
			(uint32_t) offsetof(portal_handoff_t, crc32c));

		// The reset follows immediately, and this is a plain SRAM write on a core with no write
		// buffer to speak of -- but a barrier here costs nothing and makes the ordering explicit
		// rather than a property of the compiler's mood.
		__DSB();
	}

	//----------
	bool present()
	{
		const portal_handoff_t * target = block();
		return target->magic == PORTAL_HANDOFF_MAGIC
			&& target->version == PORTAL_HANDOFF_VERSION
			&& target->crc32c == portal_crc32c((const uint8_t *) target,
				(uint32_t) offsetof(portal_handoff_t, crc32c));
	}

} // namespace Handoff
