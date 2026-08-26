#include "bl/session.hpp"
#include "bl/hw.hpp"

#include <string.h>

namespace bl {
	namespace {
		/// The erase always covers the whole v6 bank, whichever base is being written to.
		///
		/// That matters for the legacy base. Uploading a legacy-base image erases `0x08004000`
		/// upward, so the 8 kB below the legacy base is left blank -- which is precisely the
		/// condition `decideRun` requires before it will fall back to starting an application
		/// there. Erasing only from the write base would leave a stale new-base image in place to
		/// shadow the one just uploaded.
		constexpr uint32_t firstPage = config::appFirstPage;
		constexpr uint32_t lastPage = config::appLastPage;
	}

	//----------
	void
	Session::beginErase(uint32_t base)
	{
		this->writeBase = (base == config::appBaseLegacy) ? config::appBaseLegacy : config::appBase;
		this->nextPage = firstPage;
		this->erasing = true;
		this->eraseError = false;
		this->clearProgress();
	}

	//----------
	bool
	Session::eraseStep()
	{
		if(!this->erasing) {
			return true;
		}

		hw::watchdogKick();
		if(hw::flashErasePage(this->nextPage) != 0) {
			// A page that will not erase is a dead board, not a retryable condition. Stopping
			// beats looping on the same page until the watchdog fires, which would look like a
			// boot loop instead of a flash fault.
			//
			// The failure is latched rather than merely stopping the loop. Without it `begin`
			// answers `ok` -- the erase "finished", after all -- and the host streams a whole
			// image into a bank that cannot hold it, which then fails `verify` with nothing to
			// say why. This is the only path here that could corrupt an upload silently.
			this->erasing = false;
			this->eraseError = true;
			return true;
		}

		this->nextPage++;
		if(this->nextPage > lastPage) {
			this->erasing = false;
			return true;
		}
		return false;
	}

	//----------
	uint32_t
	Session::erasePages() const
	{
		return this->nextPage > firstPage ? this->nextPage - firstPage : 0;
	}

	//----------
	void
	Session::clearProgress()
	{
		this->granules.clear();
		this->written = 0;
		this->receivedBytes = 0;
	}

	//----------
	uint32_t
	Session::capacity() const
	{
		return config::appEnd - this->writeBase;
	}

	//----------
	Error
	Session::declare(uint32_t length, uint32_t crc32, uint32_t chunkBytes, uint32_t base)
	{
		if(base != config::appBase && base != config::appBaseLegacy) {
			return Error::BadParam;
		}
		const uint32_t cap = config::appEnd - base;
		if(length == 0 || length > cap || (length % config::granule) != 0) {
			return Error::BadParam;
		}
		if(chunkBytes == 0
			|| chunkBytes > config::chunkMax
			|| (chunkBytes % config::granule) != 0) {
			return Error::BadParam;
		}
		// The `map` reply carries its bitmap in a bin8, whose length is one byte. A chunk small
		// enough to need more than 255 bytes of bitmap would be answered with a *truncated* one,
		// which a host reads as "every chunk past here is missing" -- so it repairs them, asks
		// again, and gets the same answer for ever. Refusing the session up front turns a silent
		// non-terminating loop into one error at the only point where anything can still be done
		// about it. At the 128 the host uses, a full bank needs 106 bytes.
		if(((length + chunkBytes - 1) / chunkBytes) > 255u * 8u) {
			return Error::BadParam;
		}

		this->declaredLength = length;
		this->declaredCrc = crc32;
		this->declaredChunk = chunkBytes;
		this->hasDeclaration = true;
		return Error::None;
	}

	//----------
	Error
	Session::write(uint32_t offset, const uint8_t * data, uint32_t length)
	{
		if(this->erasing) {
			return Error::Busy;
		}
		if((offset % config::granule) != 0) {
			return Error::Align;
		}
		// Written without an `offset + length` sum anywhere, so a length near the top of the
		// range cannot wrap past the check it is supposed to fail.
		const uint32_t cap = this->capacity();
		if(offset > cap || length > cap - offset) {
			return Error::Bounds;
		}
		if(length == 0) {
			return Error::None;
		}

		uint32_t done = 0;
		while(done < length) {
			const uint32_t at = offset + done;
			const uint32_t index = at / config::granule;
			const uint32_t remaining = length - done;
			const uint32_t take = remaining < config::granule ? remaining : config::granule;

			if(!this->granules.get(index)) {
				// A final payload shorter than a double-word is padded with flash's erased value,
				// so the tail of an image whose length is not a multiple of 8 is deterministic.
				// The v4/v5 bootloader instead advanced a raw uint64_t* and programmed whatever
				// happened to be on the stack past the caller's buffer.
				uint8_t doubleWord[config::granule];
				memset(doubleWord, 0xFF, sizeof(doubleWord));
				memcpy(doubleWord, data + done, take);

				const uint8_t * existing = hw::flashPtr(this->writeBase + at);
				if(memcmp(existing, doubleWord, config::granule) == 0) {
					// Already holds exactly this. A duplicate frame after a reset that lost the
					// bitmap; accept it and record it rather than failing on PROGERR.
					this->granules.set(index);
				}
				else {
					for(uint32_t byte = 0; byte < config::granule; byte++) {
						if(existing[byte] != 0xFF) {
							return Error::Program;
						}
					}
					if(hw::flashProgram8(this->writeBase + at, doubleWord) != 0) {
						return Error::Program;
					}
					if(memcmp(hw::flashPtr(this->writeBase + at), doubleWord, config::granule) != 0) {
						return Error::Program;
					}
					this->granules.set(index);
				}
			}

			done += take;
		}

		this->receivedBytes += length;
		if(offset + length > this->written) {
			this->written = offset + length;
		}
		return Error::None;
	}

} // namespace bl
