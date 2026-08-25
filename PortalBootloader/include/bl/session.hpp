// An upload in progress: what has been erased, what has arrived, and what it claims to be.
//
// # The erase is incremental, and that is the point
//
// Erasing 53 pages takes something over a second. The v4/v5 bootloader did it in one blocking
// call, during which its DMA receive ring overflowed and every frame the host sent was lost --
// which is why the host had to follow `ER` with three seconds of announce frames, and why the
// erase had to be sent twice in case a board arrived late. The bootloader could not say when it
// was ready because it could not say anything.
//
// Here `beginErase()` only arms it and `eraseStep()` erases one page per main-loop pass, so
// frames are parsed between pages and the receive ring never has to hold more than one page-time
// of traffic. The `begin` verb's reply is then sent when the last page is done, which turns "wait
// three seconds and hope" into "wait for the answer".
//
// # Writes are idempotent, at the granularity flash actually works in
//
// See `bitmap.hpp`. The short version: this part refuses to program a double-word twice, and
// duplicate frames are routine, so every write consults the bitmap first.
#pragma once

#include <stdint.h>
#include <stddef.h>

#include "bl/bitmap.hpp"
#include "bl/config.hpp"
#include "bl/errors.hpp"

namespace bl {

	class Session {
	public:
		/// Arm a full-bank erase. Idempotent: re-arming an erase already in progress restarts it,
		/// which is what a repeated `ER` from a legacy host means.
		void beginErase(uint32_t base);

		/// Erase one page. Returns true when the last page is done.
		bool eraseStep();

		bool isErasing() const { return this->erasing; }
		/// Pages erased so far, for `status.prog`.
		uint32_t erasePages() const;

		/// Declare what is being uploaded. Validates the parameters; on success the session is
		/// armed and `run` will refuse an image whose CRC does not match.
		Error declare(uint32_t length, uint32_t crc32, uint32_t chunkBytes, uint32_t base);

		/// Write one payload. Any offset order; duplicates and overlaps are skipped rather than
		/// refused, because on this bus they are normal traffic rather than a mistake.
		Error write(uint32_t offset, const uint8_t * data, uint32_t length);

		/// Reset the received-data tracking without erasing.
		void clearProgress();

		/// Whether an image has been declared (a `begin` session, as opposed to a legacy upload).
		bool declared() const { return this->hasDeclaration; }

		uint32_t base() const { return this->writeBase; }
		uint32_t length() const { return this->declaredLength; }
		uint32_t crc32() const { return this->declaredCrc; }
		uint32_t chunkBytes() const { return this->declaredChunk; }
		/// Highest byte offset written this session, for `status.wp`.
		uint32_t highWater() const { return this->written; }
		/// Bytes accepted this session, for `status.n`.
		uint32_t received() const { return this->receivedBytes; }

		const Bitmap & bitmap() const { return this->granules; }

		/// The capacity available at the current write base.
		uint32_t capacity() const;

	private:
		Bitmap granules;
		uint32_t writeBase = config::appBase;
		uint32_t declaredLength = 0;
		uint32_t declaredCrc = 0;
		uint32_t declaredChunk = config::chunkMax;
		uint32_t written = 0;
		uint32_t receivedBytes = 0;
		uint32_t nextPage = 0;
		bool erasing = false;
		bool hasDeclaration = false;
	};

} // namespace bl
