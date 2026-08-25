#include "BootloaderImage.h"
#include "App.h"
#include "RS485.h"
#include "MotorDriver.h"

#include "../Logger.h"
#include "../Handoff.h"

#include "portal_crc32c.h"

#include <Arduino.h>
#include <string.h>

// The watchdog is running and cannot be stopped; the install loop below has to feed it from
// inside, not around.
#include "stm32g0xx_ll_iwdg.h"

namespace Modules {
	namespace {
		/// The chunk size a host must use. Fixed rather than negotiated: the whole transfer is at
		/// most 128 frames, so there is nothing to tune and one less thing to disagree about.
		constexpr uint32_t chunkBytes = 128;
		constexpr uint32_t chunkCount = PORTAL_BOOTLOADER_BYTES / chunkBytes;

		/// Smallest thing that could possibly be a bootloader: a vector table and a little code.
		constexpr uint32_t minimumLength = 0x100;

		/// A bootloader identifies itself by this string, and the host tooling finds it the same
		/// way. Requiring it here is a cheap guard against an application image, a settings blob,
		/// or a truncated download being written over the one thing that can recover the board.
		constexpr const char * banner = "Bootloader v";

		bool contains(const uint8_t * data, uint32_t length, const char * needle)
		{
			const uint32_t needleLength = (uint32_t) strlen(needle);
			if(needleLength == 0 || length < needleLength) {
				return false;
			}
			for(uint32_t at = 0; at + needleLength <= length; at++) {
				if(memcmp(data + at, needle, needleLength) == 0) {
					return true;
				}
			}
			return false;
		}
	}

	uint8_t BootloaderImage::buffer[PORTAL_BOOTLOADER_BYTES];
	uint8_t BootloaderImage::received[PORTAL_BOOTLOADER_BYTES / 128 / 8];

	//----------
	BootloaderImage::BootloaderImage(App * app)
	: app(app)
	{
	}

	//----------
	const char *
	BootloaderImage::getTypeName() const
	{
		return "BootloaderImage";
	}

	//----------
	bool
	BootloaderImage::processIncoming(Stream & stream)
	{
		return Base::processIncoming(stream);
	}

	//----------
	bool
	BootloaderImage::processIncomingByKey(const char * key, Stream & stream)
	{
		if(strcmp(key, "begin") == 0) {
			return this->handleBegin(stream);
		}
		if(strcmp(key, "data") == 0) {
			return this->handleData(stream);
		}
		if(strcmp(key, "commit") == 0) {
			return this->handleCommit(stream);
		}
		if(strcmp(key, "abort") == 0) {
			if(!msgpack::readNil(stream)) {
				return false;
			}
			this->state = State::Idle;
			this->receivedBytes = 0;
			memset(BootloaderImage::received, 0, sizeof(BootloaderImage::received));
			return true;
		}
		if(strcmp(key, "q") == 0) {
			if(!msgpack::readNil(stream)) {
				return false;
			}
			this->handleQuery();
			return true;
		}
		return false;
	}

	//----------
	bool
	BootloaderImage::handleBegin(Stream & stream)
	{
		// `[length, crc32c]`
		size_t arraySize;
		if(!msgpack::readArraySize(stream, arraySize) || arraySize < 2) {
			return false;
		}
		uint32_t length;
		uint32_t crc;
		if(!msgpack::readInt<uint32_t>(stream, length)
			|| !msgpack::readInt<uint32_t>(stream, crc)) {
			return false;
		}

		if(length < minimumLength
			|| length > PORTAL_BOOTLOADER_BYTES
			|| (length % 8) != 0) {
			// A length that is not a whole number of double-words cannot be programmed as one, and
			// silently rounding it is how a tail of undefined bytes ends up in flash.
			this->state = State::Rejected;
			return false;
		}

		this->declaredLength = length;
		this->declaredCrc = crc;
		this->receivedBytes = 0;
		this->state = State::Receiving;
		memset(BootloaderImage::received, 0, sizeof(BootloaderImage::received));
		// Erased-state fill, so a chunk that never arrives shows up as 0xFF rather than as
		// whatever the previous transfer left -- and fails the CRC rather than being programmed.
		memset(BootloaderImage::buffer, 0xFF, sizeof(BootloaderImage::buffer));

		log(LogLevel::Status, this->getTypeName(), "receiving a bootloader image");
		return true;
	}

	//----------
	bool
	BootloaderImage::handleData(Stream & stream)
	{
		// `[offset, bin]`
		size_t arraySize;
		if(!msgpack::readArraySize(stream, arraySize) || arraySize < 2) {
			return false;
		}
		uint32_t offset;
		if(!msgpack::readInt<uint32_t>(stream, offset)) {
			return false;
		}
		uint16_t size;
		if(!msgpack::readBinarySize(stream, size)) {
			return false;
		}

		if(this->state != State::Receiving) {
			return false;
		}
		// Bounds without a sum that could wrap past the check it is meant to fail.
		if((offset % chunkBytes) != 0
			|| size == 0
			|| size > chunkBytes
			|| offset > this->declaredLength
			|| size > this->declaredLength - offset) {
			return false;
		}

		if(!msgpack::readRaw(stream, (char *) (BootloaderImage::buffer + offset), size)) {
			return false;
		}

		const uint32_t chunk = offset / chunkBytes;
		const uint8_t bit = (uint8_t) (1u << (chunk & 7u));
		if((BootloaderImage::received[chunk >> 3] & bit) == 0) {
			BootloaderImage::received[chunk >> 3] |= bit;
			this->receivedBytes += size;
		}
		return true;
	}

	//----------
	bool
	BootloaderImage::imageIsPlausible() const
	{
		if(this->state != State::Receiving || this->declaredLength == 0) {
			return false;
		}

		// Every chunk of the declared length must have arrived.
		uint32_t chunks = 0;
		for(uint32_t at = 0; at < this->declaredLength; at += chunkBytes) {
			chunks++;
		}
		for(uint32_t chunk = 0; chunk < chunks; chunk++) {
			if((BootloaderImage::received[chunk >> 3] & (1u << (chunk & 7u))) == 0) {
				return false;
			}
		}

		if(portal_crc32c(BootloaderImage::buffer, this->declaredLength) != this->declaredCrc) {
			return false;
		}

		// A vector table that could actually start. Word 0 is the initial stack pointer, word 1
		// the reset vector; both being plausible does not prove the image is good, but either
		// being wrong proves it is not, and this is the last moment either can be checked.
		uint32_t stackPointer;
		uint32_t resetVector;
		memcpy(&stackPointer, BootloaderImage::buffer, 4);
		memcpy(&resetVector, BootloaderImage::buffer + 4, 4);
		if(stackPointer <= PORTAL_RAM_BASE || stackPointer > PORTAL_RAM_END) {
			return false;
		}
		if((resetVector & 1u) == 0) {
			return false;
		}
		if((resetVector & ~1u) < PORTAL_FLASH_BASE
			|| (resetVector & ~1u) >= PORTAL_FLASH_BASE + this->declaredLength) {
			return false;
		}

		// And it has to actually be a bootloader.
		return contains(BootloaderImage::buffer, this->declaredLength, banner);
	}

	//----------
	bool
	BootloaderImage::handleCommit(Stream & stream)
	{
		// `[stayInBootloader]`
		size_t arraySize;
		if(!msgpack::readArraySize(stream, arraySize) || arraySize < 1) {
			return false;
		}
		bool stay;
		if(!msgpack::readBool(stream, stay)) {
			return false;
		}

		// The commit gate. Everything after this point is irreversible, so this is the one command
		// in the application where an unverified frame must not be acted on.
		if(!RS485::checkChecksum()) {
			return false;
		}

		if(this->app->isRunningRoutine()) {
			// Homing or calibrating. Rewriting the bootloader means resetting, and resetting
			// mid-routine leaves the mechanism wherever it happened to be.
			log(LogLevel::Error, this->getTypeName(), "refusing: a routine is running");
			return false;
		}

		if(!this->imageIsPlausible()) {
			this->state = State::Rejected;
			log(LogLevel::Error, this->getTypeName(), "refusing: image failed its checks");
			return false;
		}

		this->state = State::Ready;

		// Answer while there is still something able to answer, and get the reply onto the wire
		// before the transmitter stops being serviced.
		RS485::sendACKEarly(true);
		log(LogLevel::Status, this->getTypeName(), "installing; do not remove power");

		this->install(stay);
		return true; // not reached
	}

	//----------
	void
	BootloaderImage::install(bool stayInBootloader)
	{
		// Motors off first. Whatever happens next, it ends in a reset, and a driver left enabled
		// through it holds current into a winding with nothing controlling it.
		this->app->motorDriverA->setEnabled(false);
		this->app->motorDriverB->setEnabled(false);

		// Leave the note the new bootloader will read: this board's address, and whether to wait
		// for an application upload or get on with starting what is already there.
		Handoff::write(this->app->id->get(), this->app->getProvisionSerial(),
			stayInBootloader ? PORTAL_HANDOFF_REQUEST_STAY : PORTAL_HANDOFF_REQUEST_NONE);

		// Every page below this image's own base, which is where the bootloader lives. Bounded by
		// `VECT_TAB_OFFSET` rather than by a constant, so a legacy-base application clears the
		// twelve pages of the old bank and a new-base one clears the eight of the new -- and
		// neither can reach its own code or the durable pages above it.
		const uint32_t pageCount = VECT_TAB_OFFSET / PORTAL_FLASH_PAGE_BYTES;

		// Interrupts stay enabled throughout.
		//
		// Flash program and erase stall the bus rather than faulting, so the handlers simply do
		// not run while an operation is in flight -- and HAL's own erase wait uses `HAL_GetTick`,
		// which needs SysTick. Masking interrupts here would turn a slow page into a watchdog
		// reset in the middle of the one sequence that must not be interrupted.
		if(HAL_FLASH_Unlock() != HAL_OK) {
			log(LogLevel::Error, this->getTypeName(), "flash unlock failed");
			NVIC_SystemReset();
		}
		__HAL_FLASH_CLEAR_FLAG(FLASH_FLAG_OPTVERR);

		for(uint32_t page = 0; page < pageCount; page++) {
			LL_IWDG_ReloadCounter(IWDG);
			FLASH_EraseInitTypeDef erase = {};
			erase.TypeErase = FLASH_TYPEERASE_PAGES;
			erase.Banks = FLASH_BANK_1;
			erase.Page = page;
			erase.NbPages = 1;
			uint32_t pageError = 0;
			if(HAL_FLASHEx_Erase(&erase, &pageError) != HAL_OK) {
				// Past this point the board has no bootloader either way. Resetting is still the
				// best move: it is what gives a probe a predictable state to attach to.
				HAL_FLASH_Lock();
				NVIC_SystemReset();
			}
		}

		for(uint32_t at = 0; at < this->declaredLength; at += 8) {
			if((at & 0x1FFu) == 0) {
				LL_IWDG_ReloadCounter(IWDG);
			}
			uint64_t doubleWord;
			memcpy(&doubleWord, BootloaderImage::buffer + at, sizeof(doubleWord));
			if(HAL_FLASH_Program(FLASH_TYPEPROGRAM_DOUBLEWORD,
				PORTAL_FLASH_BASE + at, doubleWord) != HAL_OK) {
				HAL_FLASH_Lock();
				NVIC_SystemReset();
			}
		}

		HAL_FLASH_Lock();

		// Read back through the memory map rather than through the pointer just written, and only
		// then reset. A mismatch here means the board comes back without a working bootloader, so
		// it is worth knowing before the reset rather than after.
		LL_IWDG_ReloadCounter(IWDG);
		if(memcmp((const void *) PORTAL_FLASH_BASE, BootloaderImage::buffer,
			this->declaredLength) != 0) {
			log(LogLevel::Error, this->getTypeName(), "read-back mismatch");
		}

		NVIC_SystemReset();
		while(true) {
		}
	}

	//----------
	void
	BootloaderImage::handleQuery()
	{
		// A plain reply rather than an ACK, so a host can resume an interrupted transfer instead
		// of restarting one.
		RS485::noACKRequired();
		this->app->rs485->sendBootloaderImageStatus((uint8_t) this->state,
			this->declaredLength, this->receivedBytes);
	}
}
