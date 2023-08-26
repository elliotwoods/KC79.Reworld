#pragma once

#include <stdint.h>
#include <stdbool.h>

#include "defines.h"

RAM_FUNC
bool flash_erase();

RAM_FUNC
bool flash_write(const uint8_t *src, uint32_t dst, uint32_t size);
