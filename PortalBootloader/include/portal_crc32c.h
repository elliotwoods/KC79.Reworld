/* CRC-32C (Castagnoli), the one checksum shared by everything that writes or validates a durable
 * structure on a Portal: the persistent identity and settings records, the RAM handoff block, and
 * the bootloader's `begin`/`verify` image check.
 *
 * Reflected form, polynomial 0x82F63B78, init and xorout 0xFFFFFFFF. `crc32c("123456789")` is
 * 0xE3069283 -- the standard vector, asserted on all three sides
 * (`PortalBootloader/test/test_crc`, `router_proto::crc`, `portal_swd::persistent`).
 *
 * Bitwise rather than table-driven on purpose. A 1 kB table is a real cost in a 16 kB bootloader,
 * and the only call that runs over any quantity of data is `verify`, which covers a whole 106 kB
 * application bank in roughly 70 ms at 64 MHz -- once, at the end of an upload that took tens of
 * seconds. There is nothing to win by making it faster and a page of flash to lose.
 *
 * This is the same loop that was previously private to `PortalFW/src/PersistentStorage.cpp`; that
 * file now includes this header instead, so a board and the host that provisions it cannot drift
 * apart in the one function whose disagreement would look like corrupted flash.
 */
#pragma once

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

static inline uint32_t portal_crc32c(const uint8_t * bytes, uint32_t count)
{
	uint32_t crc = 0xFFFFFFFFU;
	for (uint32_t index = 0; index < count; index++) {
		crc ^= (uint32_t) bytes[index];
		for (uint8_t bit = 0; bit < 8; bit++) {
			crc = (crc >> 1) ^ ((crc & 1U) ? 0x82F63B78U : 0U);
		}
	}
	return ~crc;
}

#ifdef __cplusplus
}
#endif
