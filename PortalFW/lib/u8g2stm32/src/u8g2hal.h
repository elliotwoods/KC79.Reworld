#pragma once
#include "U8g2lib.h"

void u8x8_stm32_init_i2c();
uint8_t u8x8_stm32_gpio_and_delay (u8x8_t * u8x8, uint8_t msg, uint8_t arg_int, void * arg_ptr);
uint8_t u8x8_byte_stm32_hw_i2c (u8x8_t * u8x8, uint8_t msg, uint8_t arg_int, void * arg_ptr);