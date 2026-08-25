// Internal interface between the STM32-only translation units.
//
// Not part of the bootloader's design surface -- `bl::hw` is. This is here so `main.cpp` can bring
// the peripherals up in the one order that works, without those functions being visible to code
// that has no business calling them twice.
#pragma once

#include <stdint.h>
#include <stddef.h>

#include "stm32g0xx.h"

#include "msgpack/Platform.hpp"

#include "bl/config.hpp"

namespace bl {
namespace target {

	/// HSE -> PLL -> 64 MHz, or HSI16 if the crystal never starts. Returns the resulting SYSCLK.
	uint32_t clockInit();

	/// Copy the vector table into SRAM and point VTOR at it.
	void vectorsToRam();

	/// GPIO, both USARTs, and the receive interrupt. `sysclk` selects the baud-rate divisor.
	void serialInit(uint32_t sysclk);

	/// Start the independent watchdog. Cannot be undone.
	void watchdogInit();

	/// The RS485 stream the bootloader parses from and replies on.
	msgpack::Stream & rs485();

	/// Consume the handoff block's run-now request, if it is holding one, and start that
	/// application. Returns if there is nothing to do.
	void handoffFastPath();

} // namespace target
} // namespace bl
