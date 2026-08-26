// The hardware seam, against the real STM32G070.
//
// The counterpart of `lib/bltest/fake_hw.cpp`. Between them they are the only two files that know
// this is a microcontroller at all.

#include "target.hpp"

#include "bl/hw.hpp"

#include "portal_crc32c.h"

#include <string.h>

/// Start of the relocated vector table, from the linker script. `extern "C"` because a linker
/// symbol has no namespace and no mangling.
extern "C" uint32_t _sramvec;

namespace bl {
namespace target {
	uint32_t ticks();
	bool ringOverran();
}
}

namespace {
	/// The handoff block, placed by the linker in the 32 bytes above the stack.
	///
	/// `NOLOAD`, so startup neither copies nor zeroes it -- which is the entire point: it has to
	/// survive the reset that carries it here.
	__attribute__((section(".handoff"), used))
	portal_handoff_t g_handoff;

	void reseal(portal_handoff_t * block)
	{
		block->crc32c = portal_crc32c((const uint8_t *) block,
			(uint32_t) offsetof(portal_handoff_t, crc32c));
	}

	bool valid(const portal_handoff_t * block)
	{
		return block->magic == PORTAL_HANDOFF_MAGIC
			&& block->version == PORTAL_HANDOFF_VERSION
			&& block->crc32c == portal_crc32c((const uint8_t *) block,
				(uint32_t) offsetof(portal_handoff_t, crc32c));
	}

	/// Hand control to the application at `base`, from a freshly reset machine.
	[[noreturn]] void enter(uint32_t base)
	{
		typedef void (*EntryPoint)(void);
		const uint32_t * table = (const uint32_t *) base;
		const EntryPoint entry = (EntryPoint) table[1];

		// The application inherits a running watchdog and expects to have to feed it. Starting it
		// here rather than leaving it to the application means a broken startup is a visible reset
		// loop instead of a board that hangs before it can be asked anything.
		bl::target::watchdogInit();

		__disable_irq();
		NVIC->ICER[0] = 0xFFFFFFFFu;
		NVIC->ICPR[0] = 0xFFFFFFFFu;
		SCB->VTOR = base;
		__DSB();
		__ISB();
		__set_MSP(table[0]);
		__enable_irq();

		entry();
		while(true) {
		}
	}
}

namespace bl {
namespace target {

	//----------
	void vectorsToRam()
	{
		// VTOR's TBLOFF field is bits [31:8], so the table has to be 256-byte aligned. The linker
		// script puts `.ram_vectors` first in RAM for that reason.
		uint32_t * ram = &_sramvec;
		const uint32_t * flash = (const uint32_t *) PORTAL_FLASH_BASE;
		for(uint32_t index = 0; index < 48u; index++) {
			ram[index] = flash[index];
		}
		SCB->VTOR = (uint32_t) ram;
		__DSB();
		__ISB();
	}

	//----------
	void watchdogInit()
	{
		// Prescaler 32, reload 4095: about 4.1 s from the ~32 kHz LSI. The same period the v4/v5
		// bootloader used, kept because the application inherits it.
		RCC->CSR |= RCC_CSR_LSION;
		while(!(RCC->CSR & RCC_CSR_LSIRDY)) {
		}
		IWDG->KR = 0xCCCCu;   // start
		IWDG->KR = 0x5555u;   // unlock the prescaler and reload registers
		IWDG->PR = 3u;        // divide by 32
		IWDG->RLR = 4095u;
		while(IWDG->SR != 0u) {
		}
		IWDG->KR = 0xAAAAu;
	}

	//----------
	void handoffFastPath()
	{
		if(!valid(&g_handoff) || g_handoff.request != PORTAL_HANDOFF_REQUEST_RUN_NOW) {
			return;
		}

		const uint32_t base = g_handoff.arg0;
		// Consume the request before acting on it. An application that faults during startup then
		// comes back to a bootloader that will stay resident rather than one that bounces it
		// straight back into the same fault.
		g_handoff.request = PORTAL_HANDOFF_REQUEST_NONE;
		reseal(&g_handoff);

		if(base != PORTAL_APP_BASE && base != PORTAL_APP_BASE_LEGACY) {
			return;
		}
		const uint32_t * table = (const uint32_t *) base;
		const uint32_t stackPointer = table[0];
		const uint32_t resetVector = table[1];
		if(stackPointer <= PORTAL_RAM_BASE || stackPointer > PORTAL_RAM_END) {
			return;
		}
		if((resetVector & 1u) == 0u) {
			return;
		}
		enter(base);
	}

} // namespace target
} // namespace bl

namespace bl {
namespace hw {

	//----------
	const uint8_t * flashPtr(uint32_t address)
	{
		return (const uint8_t *) address;
	}

	//----------
	void uid(uint32_t out[3])
	{
		const volatile uint32_t * words = (const volatile uint32_t *) PORTAL_UID_BASE;
		out[0] = words[0];
		out[1] = words[1];
		out[2] = words[2];
	}

	//----------
	uint8_t dip()
	{
		// Active low, so the switches read inverted. The application maps the same four pins the
		// same way, and a board has to answer on the same address whichever image is running.
		return (uint8_t) ((~GPIOD->IDR) & 0x0Fu);
	}

	//----------
	portal_handoff_t * handoff()
	{
		return &g_handoff;
	}

	//----------
	uint32_t millis()
	{
		return bl::target::ticks();
	}

	//----------
	void watchdogKick()
	{
		IWDG->KR = 0xAAAAu;
	}

	//----------
	bool ringOverran()
	{
		// The flag itself lives with the ISR that sets it, in SRAM. This is only the seam.
		return target::ringOverran();
	}

	//----------
	void ledToggle(Led led)
	{
		const uint32_t pin = (led == Led::Frame) ? 3u : 4u;
		GPIOB->ODR ^= (1u << pin);
	}

	//----------
	void ledSet(Led led, bool on)
	{
		const uint32_t pin = (led == Led::Frame) ? 3u : 4u;
		GPIOB->BSRR = on ? (1u << pin) : (1u << (pin + 16u));
	}

	//----------
	void logChar(char value)
	{
		while(!(USART1->ISR & USART_ISR_TXE_TXFNF)) {
		}
		USART1->TDR = (uint8_t) value;
	}

	//----------
	void logString(const char * text)
	{
		while(*text != '\0') {
			logChar(*text++);
		}
	}

	//----------
	void txDrain()
	{
		bl::target::rs485().flush();
	}

	//----------
	void reset()
	{
		NVIC_SystemReset();
		while(true) {
		}
	}

	//----------
	void runApplication(uint32_t base)
	{
		// Written as "record the intent and reset" rather than as an in-place jump.
		//
		// The v4/v5 bootloader tore its own clock tree and peripherals down with `HAL_RCC_DeInit`
		// and `HAL_DeInit` before jumping, and getting that sequence wrong is not a crash -- it is
		// an application whose `HAL_RCC_OscConfig` returns an error because the PLL it wants to
		// configure is the one currently driving SYSCLK, or a stale interrupt arriving against a
		// vector table that has just moved. After a reset every peripheral is already in exactly
		// the state the application's own initialisation expects, and none of that is reachable.
		portal_handoff_t * block = &g_handoff;
		if(!valid(block)) {
			memset(block, 0, sizeof(*block));
			block->magic = PORTAL_HANDOFF_MAGIC;
			block->version = PORTAL_HANDOFF_VERSION;
			block->id = -1;
		}
		block->request = PORTAL_HANDOFF_REQUEST_RUN_NOW;
		block->arg0 = base;
		reseal(block);

		NVIC_SystemReset();
		while(true) {
		}
	}

	//----------
	bool terminalActionsHalt()
	{
		return true;
	}

} // namespace hw
} // namespace bl
