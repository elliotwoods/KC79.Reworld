#include "flash.hpp"
#include "stm32g0xx_hal.h"
#include "Logger.hpp"

#include <stdio.h>
#include <string.h>
#include "constants.h"

const char * messageUnlockFailed = "Unlock failed";
const char * messageLockFailed = "Lock failed";

//https://community.st.com/t5/embedded-software-mcus/stm32g0-and-flash-typeprogram-fast-fail/td-p/122452
// Erase the application area
Exception flash_erase()
{
	// Unlock the flash
	if(HAL_FLASH_Unlock() != HAL_OK) {
		return Exception(messageUnlockFailed);
	}

	// Clear the flash validity flag
	__HAL_FLASH_CLEAR_FLAG(FLASH_FLAG_OPTVERR);

	// Perform erase
	logPrint("Erasing..");
	{
		FLASH_EraseInitTypeDef flashErase;
		{
			auto appOffset = (APP_FLASH_ADDRESS - FLASH_BASE);
			flashErase.TypeErase = FLASH_TYPEERASE_PAGES;
			flashErase.Banks = FLASH_BANK_1;
			flashErase.Page = appOffset / FLASH_PAGE_SIZE;
			flashErase.NbPages = APP_FLASH_SIZE / FLASH_PAGE_SIZE;
		}

		uint32_t pageError;
		if(HAL_FLASHEx_Erase(&flashErase, &pageError) != HAL_OK) {
			char message[64];
			sprintf(message, "Erase failed (page=%d)", (int) pageError);
			HAL_FLASH_Lock();
			return Exception(message);
		}

		logPrint("Done\r\n");
	}

	// Lock the flash
	if(HAL_FLASH_Lock() != HAL_OK) {
		return Exception(messageLockFailed);
	}

	return Exception::None();
}

Exception flash_write(const uint8_t *src, uint32_t dst, uint32_t size)
{
	// Firmware updates may not enter the three durable pages. Check without an overflowing
	// `dst + size` expression, and include the padded final double-word in the bound.
	const uint32_t programmedSize = (size + 7U) & ~7U;
	if(dst < APP_FLASH_ADDRESS || dst > APP_FLASH_END
		|| programmedSize > APP_FLASH_END - dst) {
		return Exception("Write outside app partition");
	}
	// Unlock the flash
	if(HAL_FLASH_Unlock() != HAL_OK) {
		return Exception(messageUnlockFailed);
	}

	// Clear the flash validity flag
	__HAL_FLASH_CLEAR_FLAG(FLASH_FLAG_OPTVERR);

	// A final chunk whose length isn't a multiple of 8 is padded with 0xFF (flash's
	// erased-state value) rather than reading past the end of the caller's buffer for
	// the remaining bytes of the double-word. The original implementation advanced a
	// raw uint64_t* from `src` to `src + size` regardless of 8-byte alignment, so any
	// non-aligned final chunk read past the caller's buffer (here, always a stack VLA
	// in FWUpdateApp::processIncoming) and programmed whatever garbage was there.
	uint32_t bytesWritten = 0;

	while(bytesWritten < size) {
		uint64_t doubleWord = 0xFFFFFFFFFFFFFFFFULL;
		uint32_t remaining = size - bytesWritten;
		uint32_t chunk = remaining < sizeof(uint64_t) ? remaining : sizeof(uint64_t);
		memcpy(&doubleWord, src + bytesWritten, chunk);

		if(HAL_FLASH_Program(FLASH_TYPEPROGRAM_DOUBLEWORD, dst, doubleWord) != HAL_OK) {
			HAL_FLASH_Lock();

			char message[64];
			sprintf(message
					, "Write fail 0x%X, error=0x%X"
					, (unsigned int) dst
					, (unsigned int) HAL_FLASH_GetError());
			return Exception(message);
		}

		bytesWritten += chunk;
		dst += sizeof(uint64_t);
	}


	// Lock the flash
	if(HAL_FLASH_Lock() != HAL_OK) {
		return Exception(messageLockFailed);
	}

	return Exception::None();
}

Exception flash_write_fast(const uint8_t *src, uint32_t dst, uint32_t size)
{
	// Unlock the flash
	if(HAL_FLASH_Unlock() != HAL_OK) {
		return Exception(messageUnlockFailed);
	}

	// Clear the flash validity flag
	__HAL_FLASH_CLEAR_FLAG(FLASH_FLAG_OPTVERR);

	// Try just to write a small thing
//	if(HAL_FLASH_Program(FLASH_TYPEPROGRAM_DOUBLEWORD, dst, 0xF0F0F0F0UL) != HAL_OK) {
//		return Exception("Test failed");
//	}


	auto writeDestination = dst;

	// Write all the rows
	while(writeDestination < dst + size) {
		if (HAL_FLASH_Program(FLASH_TYPEPROGRAM_FAST
		    		, writeDestination
					, (uint64_t) src) != HAL_OK)
		{
			HAL_FLASH_Lock();

			char message[64];
			sprintf(message
					, "Write fail 0x%X, error=%X"
					, (unsigned int) src
					, HAL_FLASH_GetError());
			return Exception(message);
		}

		// Advance to next row
		writeDestination += FLASH_ROW_SIZE;
	}


	// Lock the flash
	if(HAL_FLASH_Lock() != HAL_OK) {
		return Exception(messageLockFailed);
	}

	return Exception::None();
}
