// SYSCLK, and the one decision in it that is not a transcription.
//
// The clock tree is the same one the v5 bootloader configured through the HAL: an 8 MHz crystal,
// PLLM=1, PLLN=16, PLLR=2, giving 64 MHz, at flash latency 2. It matches the application's, which
// matters because the application inherits a running IWDG and expects its own timing.
//
// What is different: `HAL_RCC_OscConfig` calls `Error_Handler()` when the crystal does not start,
// and `Error_Handler()` is an infinite loop with interrupts disabled. A board whose crystal has
// failed therefore goes completely dark -- no bootloader, no RS485, no way to tell it apart from
// a dead MCU without a probe. Here a crystal that does not start falls back to the internal
// 16 MHz oscillator and carries on: slower, and still able to receive firmware.

#include "target.hpp"

namespace bl {
namespace target {
	namespace {
		/// Long enough for any crystal that is going to start at all, counted in loop iterations
		/// because SysTick is not running yet.
		constexpr uint32_t hseTimeout = 2'000'000;
	}

	//----------
	uint32_t
	clockInit()
	{
		// The regulator has to be at scale 1 before the core may run above 16 MHz.
		RCC->APBENR1 |= RCC_APBENR1_PWREN;
		(void) RCC->APBENR1;

		// Two wait states, set *before* the clock speeds up. The other order is a fetch at 64 MHz
		// with one wait state, which does not fail predictably.
		uint32_t acr = FLASH->ACR;
		acr &= ~FLASH_ACR_LATENCY_Msk;
		acr |= (2u << FLASH_ACR_LATENCY_Pos);
		FLASH->ACR = acr;
		while((FLASH->ACR & FLASH_ACR_LATENCY_Msk) != (2u << FLASH_ACR_LATENCY_Pos)) {
		}

		RCC->CR |= RCC_CR_HSEON;
		bool crystal = false;
		for(uint32_t spins = 0; spins < hseTimeout; spins++) {
			if(RCC->CR & RCC_CR_HSERDY) {
				crystal = true;
				break;
			}
		}

		if(!crystal) {
			// HSI16 it is. One wait state is enough at 16 MHz, but leaving two costs nothing and
			// keeps this path from having a second thing that can be wrong.
			RCC->CR |= RCC_CR_HSION;
			while(!(RCC->CR & RCC_CR_HSIRDY)) {
			}
			RCC->CFGR &= ~RCC_CFGR_SW_Msk;
			while((RCC->CFGR & RCC_CFGR_SWS_Msk) != 0u) {
			}
			return 16'000'000u;
		}

		// PLLM = 1, PLLN = 16, PLLR = 2: 8 MHz / 1 * 16 / 2 = 64 MHz.
		RCC->CR &= ~RCC_CR_PLLON;
		while(RCC->CR & RCC_CR_PLLRDY) {
		}
		RCC->PLLCFGR = RCC_PLLCFGR_PLLSRC_HSE
			| (0u << RCC_PLLCFGR_PLLM_Pos)      // division by 1
			| (16u << RCC_PLLCFGR_PLLN_Pos)
			| (1u << RCC_PLLCFGR_PLLR_Pos)      // division by 2
			| RCC_PLLCFGR_PLLREN;
		RCC->CR |= RCC_CR_PLLON;
		while(!(RCC->CR & RCC_CR_PLLRDY)) {
		}

		// AHB and APB1 undivided.
		RCC->CFGR &= ~(RCC_CFGR_HPRE_Msk | RCC_CFGR_PPRE_Msk);
		RCC->CFGR = (RCC->CFGR & ~RCC_CFGR_SW_Msk) | RCC_CFGR_SW_1; // PLLRCLK
		while((RCC->CFGR & RCC_CFGR_SWS_Msk) != RCC_CFGR_SWS_1) {
		}

		return 64'000'000u;
	}

} // namespace target
} // namespace bl
