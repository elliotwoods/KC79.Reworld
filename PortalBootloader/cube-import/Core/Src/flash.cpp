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
			flashErase.NbPages = (FLASH_SIZE - appOffset) / FLASH_PAGE_SIZE;
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
	// Unlock the flash
	if(HAL_FLASH_Unlock() != HAL_OK) {
		return Exception(messageUnlockFailed);
	}

	// Clear the flash validity flag
	__HAL_FLASH_CLEAR_FLAG(FLASH_FLAG_OPTVERR);

	auto start = (uint64_t*) src;
	auto end = (uint64_t*) (src + size);
	auto data = start;

	while(data < end) {
		// I think the data needs to be aligned here or something like that?
		uint64_t doubleWord;
		memcpy(&doubleWord, data, sizeof(uint64_t));

		if(HAL_FLASH_Program(FLASH_TYPEPROGRAM_DOUBLEWORD, dst, doubleWord) != HAL_OK) {
			HAL_FLASH_Lock();

			char message[64];
			sprintf(message
					, "Write fail 0x%X, error=0x%X"
					, (unsigned int) dst
					, (unsigned int) HAL_FLASH_GetError());
			return Exception(message);
		}

		data++;
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
