extern "C" {
	#include "flash.h"
	#include "bootloader.h"
	#include "watchdog.h"
	#include "logger.h"
}


#include <Arduino.h>
#include "stm32g0xx_ll_iwdg.h"

unsigned char * flash_memory = (unsigned char *) BOOTLOADER_POSITION;

void setup() {
	HAL_NVIC_DisableIRQ(DMA1_Channel1_IRQn);
	
	slowDownWatchdog();

	feedWatchdog();

	logInit();
	logPrintln("Bootloader updater");
	
	logPrintln("Checking bootloader...");
	{
		bool isCorrect = true;

		for(uint32_t i=0; i<BOOTLOADER_SIZE; i++) {
			if(_bootloader_bin[i] != flash_memory[i]) {
				isCorrect = false;
				break;
			}

			if(i % 256 == 0) {
				feedWatchdog();
			}
		}

		if(isCorrect) {
			logPrintln("Bootloader already correct");
			return;
		}
	}

	logPrintln("Erasing flash:");
	flash_erase();

	feedWatchdog();

	logPrintln("Overwriting bootloader:");
	{
		uint32_t pageSize = FLASH_PAGE_SIZE;
		for(uint32_t i=0; i<BOOTLOADER_SIZE; i+=pageSize) {
			flash_write(_bootloader_bin, BOOTLOADER_POSITION + i, pageSize);
			feedWatchdog();
			logPrint(".");
		}
	}
	

	logPrintln("Done");
}

void loop() {
	logPrintln("Rebooting...");

	for(int i=0; i<10; i++) {
		feedWatchdog();
		HAL_Delay(100);
	}

	NVIC_SystemReset();
}