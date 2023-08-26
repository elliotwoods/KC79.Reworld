
#include "flash.h"

#include "stm32g0xx_hal.h"
#include "stm32g0xx_hal_flash.h"

#include "logger.h"
#include "watchdog.h"
#include "bootloader.h"

#include <stdio.h>
#include <string.h>

RAM_FUNC
bool flash_erase()
{
	// Unlock the flash
	if(HAL_FLASH_Unlock() != HAL_OK) {
		logPrintln("Can't unlock flash");
		return false;
	}

	// Clear the flash validity flag
	__HAL_FLASH_CLEAR_FLAG(FLASH_FLAG_OPTVERR);

	// Perform erase
	logPrintln("Erasing page by page:");
	{
		feedWatchdog();

		uint32_t pageError;
		uint32_t pages = BOOTLOADER_SIZE / FLASH_PAGE_SIZE;

		FLASH_EraseInitTypeDef flashErase;
		{
			flashErase.TypeErase = FLASH_TYPEERASE_PAGES;
			flashErase.Banks = FLASH_BANK_1;
			flashErase.Page = 100 * 1024 / FLASH_PAGE_SIZE;
			flashErase.NbPages = BOOTLOADER_SIZE / FLASH_PAGE_SIZE;
		}

		pageError = HAL_FLASHEx_Erase(&flashErase, &pageError);
		if(pageError != HAL_OK) {
			char message[64];
			sprintf(message, "Erase failed (page=%d, error=%X)"
				, (int) flashErase.Page
				, (int) pageError);
			logPrintln(message);
			HAL_FLASH_Lock();
			return false;
		}

		feedWatchdog();
		logPrint(".");

		logPrintln("Done\r\n");
	}

	// Lock the flash
	if(HAL_FLASH_Lock() != HAL_OK) {
		logPrintln("Can't lock flash");
		return false;
	}

	return true;
}

RAM_FUNC
bool flash_write(const uint8_t *src, uint32_t dst, uint32_t size)
{
	// Unlock the flash
	if(HAL_FLASH_Unlock() != HAL_OK) {
		logPrintln("Can't unlock flash");
		return false;
	}

	// Clear the flash validity flag
	__HAL_FLASH_CLEAR_FLAG(FLASH_FLAG_OPTVERR);

	uint64_t * start = (uint64_t*) src;
	uint64_t * end = (uint64_t*) (src + size);
	uint64_t * data = start;

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
			logPrintln(message);
			return false;
		}

		data++;
		dst += sizeof(uint64_t);
	}


	// Lock the flash
	if(HAL_FLASH_Lock() != HAL_OK) {
		logPrintln("Can't lock flash");
		return false;
	}

	return true;
}