// Which 8-byte double-words of the application bank have been written this session.
//
// # Why granules rather than chunks
//
// The obvious design tracks the host's chunks: 848 bits for a full bank at 128 bytes a chunk, 106
// bytes of RAM. It is wrong for two reasons, and both of them are things that actually happen on
// this bus.
//
// A host may repair a failed upload with a *different* chunk size than it started with, or a
// legacy host may interleave 32-byte frames with a v6 host's 128-byte ones. A chunk bitmap has no
// way to represent "the first half of chunk 4 arrived".
//
// More importantly, flash on this part refuses to program a double-word twice -- reprogramming an
// already-written one raises PROGERR even when the value is identical. Duplicate frames are
// routine (the legacy host sends every frame twice by default), so the bootloader has to know, at
// double-word resolution, what it has already written. Tracking at the granularity flash actually
// works in makes duplicates free rather than fatal.
//
// The cost is 1,696 bytes of .bss for a 108,544-byte bank. Chunk bits are derived from it on
// demand, which also means `map` can answer at whatever granularity the host asks for.
#pragma once

#include <stdint.h>
#include <stddef.h>

#include "bl/config.hpp"

namespace bl {

	class Bitmap {
	public:
		void clear();

		bool get(uint32_t index) const;
		void set(uint32_t index);

		/// Whether every index in `[from, to)` is set. `to <= config::granuleCount`.
		bool allSet(uint32_t from, uint32_t to) const;

		/// How many indices are set. Only used for diagnostics, so it counts rather than caches.
		uint32_t count() const;

		/// Render chunk bits for an image of `length` bytes at `chunkBytes` per chunk.
		///
		/// Bit `i` of the output is set when *every* granule of chunk `i` has been written, so a
		/// partially-received chunk reads as missing and the host resends the whole thing. LSB
		/// first within each byte, matching the repeater's OTA bitmap so a host can share one
		/// repair loop between them.
		///
		/// Returns the number of bytes written to `out`, or 0 if it would not fit.
		size_t renderChunks(uint32_t chunkBytes, uint32_t length, uint8_t * out, size_t outSize) const;

	private:
		uint8_t bits[config::bitmapBytes];
	};

} // namespace bl
