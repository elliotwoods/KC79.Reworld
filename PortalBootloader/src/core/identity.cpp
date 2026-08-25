#include "bl/identity.hpp"
#include "bl/hw.hpp"

#include "portal_crc32c.h"

#include <string.h>

namespace bl {
	namespace {
		// The record header, from PortalFW/src/PersistentStorage.cpp. Field offsets rather than a
		// struct because this is a byte layout on flash written by a different toolchain, and a
		// struct would invite someone to add padding to it.
		constexpr uint64_t magic = 0x313030565250434BULL; // "KCPRV001", little-endian
		constexpr uint16_t schema = 1;
		constexpr uint16_t kindIdentity = 1;
		constexpr uint32_t identityPayloadBytes = 4;
		constexpr uint32_t recordBytes = 64;
		constexpr uint32_t recordsPerPage = config::pageBytes / recordBytes;
		constexpr uint32_t crcOffset = 60;

		uint16_t get16(const uint8_t * bytes, uint32_t at) {
			return (uint16_t) ((uint16_t) bytes[at] | ((uint16_t) bytes[at + 1] << 8));
		}

		uint32_t get32(const uint8_t * bytes, uint32_t at) {
			return (uint32_t) bytes[at]
				| ((uint32_t) bytes[at + 1] << 8)
				| ((uint32_t) bytes[at + 2] << 16)
				| ((uint32_t) bytes[at + 3] << 24);
		}

		uint64_t get64(const uint8_t * bytes, uint32_t at) {
			return (uint64_t) get32(bytes, at) | ((uint64_t) get32(bytes, at + 4) << 32);
		}

		bool erased(const uint8_t * bytes) {
			for(uint32_t index = 0; index < recordBytes; index++) {
				if(bytes[index] != 0xFF) {
					return false;
				}
			}
			return true;
		}
	}

	//----------
	Identity
	scanIdentityPage(const uint8_t * page, const uint32_t ownUid[3])
	{
		Identity result;

		for(uint32_t slot = 0; slot < recordsPerPage; slot++) {
			const uint8_t * bytes = page + slot * recordBytes;
			if(erased(bytes)) {
				continue;
			}
			if(get64(bytes, 0) != magic
				|| get16(bytes, 8) != schema
				|| get16(bytes, 10) != kindIdentity
				|| get32(bytes, 16) != identityPayloadBytes
				|| get32(bytes, crcOffset) != portal_crc32c(bytes, crcOffset)) {
				continue;
			}

			const uint32_t serial = get32(bytes, 32);
			// 0 and 0xFFFFFFFF are both rejected by the application's reader too: one is the
			// erased state of a partially written record, the other is what a zeroed page looks
			// like, and neither is a serial anyone was ever issued.
			if(serial == 0 || serial == 0xFFFFFFFFu) {
				continue;
			}

			if(get32(bytes, 20) != ownUid[0]
				|| get32(bytes, 24) != ownUid[1]
				|| get32(bytes, 28) != ownUid[2]) {
				result.foreignUid = true;
				continue;
			}

			// The journal is append-only, so the newest record is the one with the highest
			// generation, not the one in the highest slot -- a compaction rewrites slot 0.
			const uint32_t generation = get32(bytes, 12);
			if(!result.valid || generation > result.generation) {
				result.valid = true;
				result.generation = generation;
				result.serial = serial;
			}
		}

		return result;
	}

	//----------
	Identity
	readIdentity()
	{
		uint32_t ownUid[3];
		hw::uid(ownUid);
		return scanIdentityPage(hw::flashPtr(PORTAL_PERSIST_IDENTITY), ownUid);
	}

} // namespace bl
