// Bring-up order, and then a loop that does not sleep.
//
// The order is the design. `handoffFastPath()` runs before a single peripheral is touched, because
// the way this bootloader starts an application is to write its intent to a RAM block and reset --
// so on that path the machine is already in exactly the state the application expects, and the
// less this file has done to it, the better.
//
// The loop has no delay in it. The v4/v5 bootloader called `HAL_Delay(10)` every pass, which put a
// 10 ms floor on how fast it could answer anything and meant its receive ring had to absorb 10 ms
// of traffic between polls. There is nothing to wait for here: `tick()` returns immediately when
// there is no frame, and the one thing that does take time -- erasing a page -- is exactly the
// thing that has to be interleaved with reception rather than blocked on.

#include "target.hpp"

#include "bl/bootloader.hpp"
#include "bl/hw.hpp"

int main(void)
{
	// Before anything: were we asked to start an application?
	bl::target::handoffFastPath();

	const uint32_t sysclk = bl::target::clockInit();

	// The vector table has to be in SRAM before any interrupt is enabled, so the receive handler
	// can run while flash is being erased.
	bl::target::vectorsToRam();
	// A constant per clock rather than `sysclk / 1000`: a runtime division here would link in
	// __udivsi3, several hundred bytes of libgcc, for a value that is known at compile time.
	SysTick_Config(sysclk >= 32'000'000u ? 64'000u : 16'000u);
	NVIC_SetPriority(SysTick_IRQn, 1);

	bl::target::serialInit(sysclk);
	bl::target::watchdogInit();

	static bl::Bootloader bootloader(bl::target::rs485());
	bootloader.begin(bl::hw::millis());

	while(true) {
		bl::hw::watchdogKick();
		bootloader.tick(bl::hw::millis());
	}
}
