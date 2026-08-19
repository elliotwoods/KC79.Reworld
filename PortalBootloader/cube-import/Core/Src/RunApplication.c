#include "stm32g0xx_hal.h"
#include "RunApplication.h"

#include "constants.h"

void run_application()
{
#if 0
	void (*app_reset_handler)(void) = (void*)(*((volatile uint32_t*)(APP_FLASH_ADDRESS + 4U)));
#else
	typedef void (*app_reset_handler_pointer)(void);
	__IO uint32_t* app_vect_table = (__IO uint32_t*) APP_FLASH_ADDRESS;
	app_reset_handler_pointer app_reset_handler = (app_reset_handler_pointer) *(app_vect_table + 1);

    HAL_RCC_DeInit();
    HAL_DeInit();

    SysTick->CTRL = 0;
    SysTick->LOAD = 0;
    SysTick->VAL  = 0;

	// HAL_RCC_DeInit uses HAL_GetTick while waiting for clock transitions, so interrupts must
	// remain enabled through the HAL teardown. Mask only the actual NVIC/VTOR/MSP handoff window.
	// Otherwise a marginal clock transition can wait forever and the live IWDG resets the part
	// before the application vector table is ever installed.
	__disable_irq();

	// Do not carry bootloader interrupt enables or pending requests into the application. Keeping
	// PRIMASK set protects the VTOR/MSP transition; clearing NVIC state makes it safe to restore
	// the reset-time interrupt state immediately before entering the application's reset handler.
	NVIC->ICER[0] = 0xFFFFFFFFU;
	NVIC->ICPR[0] = 0xFFFFFFFFU;

	SCB->VTOR = (uint32_t) app_vect_table;
	__DSB();
	__ISB();

	__set_MSP(*app_vect_table);
	__enable_irq();
#endif

	app_reset_handler();
}
