#include "bl/image.hpp"
#include "bl/hw.hpp"

#include "portal_crc32c.h"

#include <string.h>

namespace bl {
	namespace {
		uint32_t word(uint32_t address) {
			uint32_t value;
			memcpy(&value, hw::flashPtr(address), sizeof(value));
			return value;
		}
	}

	//----------
	bool
	vectorTableValid(uint32_t base)
	{
		const uint32_t stackPointer = word(base);
		const uint32_t resetVector = word(base + 4);

		// The stack grows down, so the top of SRAM is a legal initial value -- `ramEnd` itself is
		// what a linker emits for `_estack`. Anything outside SRAM, or misaligned, is not a stack.
		if(stackPointer <= config::ramBase || stackPointer > config::ramEnd) {
			return false;
		}
		if((stackPointer & 3u) != 0) {
			return false;
		}

		// Thumb bit set, and pointing into this image's own bank. `& ~1` because the bit is not
		// part of the address.
		if((resetVector & 1u) == 0) {
			return false;
		}
		const uint32_t entry = resetVector & ~1u;
		return entry >= base && entry < config::appEnd;
	}

	//----------
	const portal_app_descriptor_t *
	descriptorAt(uint32_t base)
	{
		const uint8_t * bytes = hw::flashPtr(base + PORTAL_APP_DESCRIPTOR_OFFSET);
		if(memcmp(bytes, PORTAL_APP_DESCRIPTOR_MAGIC, 8) != 0) {
			return nullptr;
		}
		return reinterpret_cast<const portal_app_descriptor_t *>(bytes);
	}

	//----------
	bool
	regionBlank(uint32_t address, uint32_t length)
	{
		const uint8_t * bytes = hw::flashPtr(address);
		for(uint32_t index = 0; index < length; index++) {
			if(bytes[index] != 0xFF) {
				return false;
			}
		}
		return true;
	}

	//----------
	uint32_t
	crcOverFlash(uint32_t base, uint32_t length)
	{
		// Chunked so the watchdog gets reloaded on the way through. 2 kB at a time is arbitrary
		// except that it is one page, and one page is about 1.3 ms of CRC at 64 MHz.
		constexpr uint32_t step = 2048;
		uint32_t crc = 0xFFFFFFFFu;
		uint32_t done = 0;
		while(done < length) {
			hw::watchdogKick();
			uint32_t take = length - done;
			if(take > step) {
				take = step;
			}
			// portal_crc32c() computes a whole message, so the running form is inlined here
			// rather than calling it per chunk. The polynomial and the pre/post inversion are
			// the same; the header keeps the single-shot version that everything else uses.
			const uint8_t * bytes = hw::flashPtr(base + done);
			for(uint32_t index = 0; index < take; index++) {
				crc ^= (uint32_t) bytes[index];
				for(uint8_t bit = 0; bit < 8; bit++) {
					crc = (crc >> 1) ^ ((crc & 1u) ? 0x82F63B78u : 0u);
				}
			}
			done += take;
		}
		hw::watchdogKick();
		return ~crc;
	}

	//----------
	RunDecision
	decideRun(uint32_t sessionLength, uint32_t sessionCrc, uint32_t sessionBase)
	{
		RunDecision decision;

		// 1. A session that declared a CRC is the strongest statement available about whether the
		//    image is complete, so it is checked before anything about the image's shape. An
		//    upload that lost its last frame leaves a perfectly plausible vector table behind.
		if(sessionLength != 0 && sessionCrc != 0) {
			if(crcOverFlash(sessionBase, sessionLength) != sessionCrc) {
				decision.error = Error::ImageCrc;
				return decision;
			}
		}

		// 2. The new bank, if anything at all has been programmed into it.
		//
		//    "Anything at all" is measured over the first 8 kB rather than the vector table alone,
		//    so a half-finished upload -- which has a valid-looking vector table and nothing
		//    behind it -- cannot make the bootloader skip the legacy fallback and start rubbish.
		if(!regionBlank(config::appBase, 0x2000)) {
			if(!vectorTableValid(config::appBase)) {
				decision.error = Error::NoApp;
				return decision;
			}
			const portal_app_descriptor_t * descriptor = descriptorAt(config::appBase);
			if(descriptor == nullptr) {
				// An image at the new base with no descriptor is almost always a legacy-base
				// image that a legacy host uploaded to the wrong place. Refusing it is what turns
				// that from a hard fault into a diagnosable state.
				decision.error = Error::DescriptorMissing;
				return decision;
			}
			if(descriptor->app_base != config::appBase) {
				decision.error = Error::DescriptorBase;
				return decision;
			}
			decision.ok = true;
			decision.base = config::appBase;
			decision.error = Error::None;
			return decision;
		}

		// 3. The legacy bank. Reached only when the new one is untouched, which is exactly the
		//    state a board is in immediately after its bootloader has been replaced in the field
		//    and before its application has been re-uploaded. Without this the update would be a
		//    flag day: every board would go dark between the two steps.
		if(!vectorTableValid(config::appBaseLegacy)) {
			decision.error = Error::NoApp;
			return decision;
		}
		const portal_app_descriptor_t * legacy = descriptorAt(config::appBaseLegacy);
		if(legacy != nullptr && legacy->app_base != config::appBaseLegacy) {
			decision.error = Error::DescriptorBase;
			return decision;
		}
		decision.ok = true;
		decision.base = config::appBaseLegacy;
		decision.error = Error::None;
		return decision;
	}

} // namespace bl
