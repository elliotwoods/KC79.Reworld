#pragma once
#include <stdint.h>
#include "Exception.hpp"

#define FLASH_ROW_SIZE 256U

Exception flash_erase();
Exception flash_write_fast(const uint8_t *src, uint32_t dst, uint32_t size);
Exception flash_write(const uint8_t *src, uint32_t dst, uint32_t size);
