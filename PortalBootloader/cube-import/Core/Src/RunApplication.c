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

	SCB->VTOR = (uint32_t) app_vect_table;

	__set_MSP(*app_vect_table);
#endif

	app_reset_handler();
}
