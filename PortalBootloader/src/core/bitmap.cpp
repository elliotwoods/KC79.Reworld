#include "bl/bitmap.hpp"

#include <string.h>

namespace bl {

	//----------
	void
	Bitmap::clear()
	{
		memset(this->bits, 0, sizeof(this->bits));
	}

	//----------
	bool
	Bitmap::get(uint32_t index) const
	{
		if(index >= config::granuleCount) {
			return false;
		}
		return (this->bits[index >> 3] & (1u << (index & 7u))) != 0;
	}

	//----------
	void
	Bitmap::set(uint32_t index)
	{
		if(index >= config::granuleCount) {
			return;
		}
		this->bits[index >> 3] |= (uint8_t) (1u << (index & 7u));
	}

	//----------
	bool
	Bitmap::allSet(uint32_t from, uint32_t to) const
	{
		if(to > config::granuleCount) {
			to = config::granuleCount;
		}
		// Byte at a time across the middle. A full bank is 13,568 granules, and `map` walks all of
		// them for every chunk; bit-at-a-time would be 13,568 iterations per reply on a 64 MHz
		// M0+, which is measurable against the host's reply timeout when repeated per chunk.
		while(from < to && (from & 7u) != 0) {
			if(!this->get(from++)) {
				return false;
			}
		}
		while(from + 8 <= to) {
			if(this->bits[from >> 3] != 0xFF) {
				return false;
			}
			from += 8;
		}
		while(from < to) {
			if(!this->get(from++)) {
				return false;
			}
		}
		return true;
	}

	//----------
	uint32_t
	Bitmap::count() const
	{
		uint32_t total = 0;
		for(size_t index = 0; index < sizeof(this->bits); index++) {
			uint8_t byte = this->bits[index];
			while(byte) {
				total += byte & 1u;
				byte = (uint8_t) (byte >> 1);
			}
		}
		return total;
	}

	//----------
	size_t
	Bitmap::renderChunks(uint32_t chunkBytes, uint32_t length, uint8_t * out, size_t outSize) const
	{
		if(chunkBytes == 0 || chunkBytes % config::granule != 0 || length > config::appCap) {
			return 0;
		}

		// Counted rather than divided. `chunkBytes` is a wire value, so `length / chunkBytes` is a
		// genuine runtime division, and on a Cortex-M0+ that is a call into libgcc's `__udivsi3`
		// -- 266 bytes of it, in an image with a hard size limit, to avoid a loop that runs at
		// most 848 times once per `map` reply.
		uint32_t chunks = 0;
		for(uint32_t at = 0; at < length; at += chunkBytes) {
			chunks++;
		}
		const size_t bytes = (chunks + 7u) / 8u;
		if(bytes > outSize) {
			return 0;
		}
		memset(out, 0, bytes);

		const uint32_t granulesPerChunk = chunkBytes / config::granule;
		const uint32_t lastGranule = (length + config::granule - 1u) / config::granule;
		for(uint32_t chunk = 0; chunk < chunks; chunk++) {
			const uint32_t from = chunk * granulesPerChunk;
			uint32_t to = from + granulesPerChunk;
			if(to > lastGranule) {
				to = lastGranule;
			}
			if(this->allSet(from, to)) {
				out[chunk >> 3] |= (uint8_t) (1u << (chunk & 7u));
			}
		}
		return bytes;
	}

} // namespace bl
