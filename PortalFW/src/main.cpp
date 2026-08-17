#include <Arduino.h>
#include "Modules/App.h"

Modules::App app;

/**
 * @brief Proof, to an external debugger, that this loop is actually turning.
 *
 * The flasher's run-check reads this twice a few hundred milliseconds apart and calls the board
 * good only if the value moved. It cannot ask the question any other way: ARMv6-M has no way to
 * read the program counter without halting the core, and halting a board to find out whether it
 * is running is a contradiction. A board stuck in HardFault_Handler, spinning in a watchdog reset
 * loop, or sitting in the system ROM bootloader all present a perfectly healthy debug port.
 *
 * `volatile` so it survives optimisation -- nothing in this firmware ever reads it, and without
 * the qualifier the increment is dead code the compiler is entitled to delete.
 *
 * Not `static`: the address is resolved from the symbol table of firmware.elf when a flash bundle
 * is built, so the symbol has to have external linkage and keep this name.
 */
volatile uint32_t g_liveness_counter = 0;

/**
 * @brief System Clock Configuration
 * @retval None
 */
void SystemClock_Config(void)
{
	RCC_OscInitTypeDef RCC_OscInitStruct = {0};
	RCC_ClkInitTypeDef RCC_ClkInitStruct = {0};

	/** Configure the main internal regulator output voltage
	 */
	HAL_PWREx_ControlVoltageScaling(PWR_REGULATOR_VOLTAGE_SCALE1);

	/** Initializes the RCC Oscillators according to the specified parameters
	 * in the RCC_OscInitTypeDef structure.
	 */
	RCC_OscInitStruct.OscillatorType = RCC_OSCILLATORTYPE_HSE;
	RCC_OscInitStruct.HSEState = RCC_HSE_ON;
	RCC_OscInitStruct.PLL.PLLState = RCC_PLL_ON;
	RCC_OscInitStruct.PLL.PLLSource = RCC_PLLSOURCE_HSE;
	RCC_OscInitStruct.PLL.PLLM = RCC_PLLM_DIV1;
	RCC_OscInitStruct.PLL.PLLN = 16;
	RCC_OscInitStruct.PLL.PLLP = RCC_PLLP_DIV2;
	RCC_OscInitStruct.PLL.PLLR = RCC_PLLR_DIV2;
	if (HAL_RCC_OscConfig(&RCC_OscInitStruct) != HAL_OK)
	{
		Error_Handler();
	}

	/** Initializes the CPU, AHB and APB buses clocks
	 */
	RCC_ClkInitStruct.ClockType = RCC_CLOCKTYPE_HCLK | RCC_CLOCKTYPE_SYSCLK | RCC_CLOCKTYPE_PCLK1;
	RCC_ClkInitStruct.SYSCLKSource = RCC_SYSCLKSOURCE_PLLCLK;
	RCC_ClkInitStruct.AHBCLKDivider = RCC_SYSCLK_DIV1;
	RCC_ClkInitStruct.APB1CLKDivider = RCC_HCLK_DIV1;

	if (HAL_RCC_ClockConfig(&RCC_ClkInitStruct, FLASH_LATENCY_2) != HAL_OK)
	{
		Error_Handler();
	}
}

void setup() {
	// This breaks our RS485 comms so we disable it for now
	SystemClock_Config();

	// LED's
	pinMode(PB3, OUTPUT);
	pinMode(PB4, OUTPUT);

	app.setup();
}

void loop() {
	// First thing in the loop, so it advances even if app.update() is where a fault would be.
	// A counter that only moved on a healthy update() would answer a narrower question than the
	// one being asked, which is simply "is this loop turning".
	g_liveness_counter++;

	app.update();

	// We need some delay to allow for bigger numbers in MotionControl (otherwise can't accelerate)
	HAL_Delay(1);
}