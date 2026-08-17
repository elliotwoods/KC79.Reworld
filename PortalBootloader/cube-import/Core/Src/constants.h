#pragma once

#define BOOTLOADER_SIZE (24U * 0x400U)
#define APP_FLASH_ADDRESS (0x08000000U + BOOTLOADER_SIZE)

// Note max frame with checksum is 255 bytes because of uint8_t
#define FW_FRAME_SIZE 128
#define FW_CHECKSUM_SIZE 2
