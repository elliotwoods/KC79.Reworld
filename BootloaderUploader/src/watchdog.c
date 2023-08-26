#include "watchdog.h"

#include "stm32g0xx_ll_iwdg.h"

RAM_FUNC
void slowDownWatchdog() {
	LL_IWDG_SetPrescaler(IWDG, LL_IWDG_PRESCALER_256);
}

RAM_FUNC
void feedWatchdog() {
	LL_IWDG_ReloadCounter(IWDG);
}
