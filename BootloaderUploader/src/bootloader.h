#pragma once

#define BOOTLOADER_SIZE 28672UL
#define BOOTLOADER_POSITION FLASH_BASE

extern const unsigned char _bootloader_bin[BOOTLOADER_SIZE + 1];