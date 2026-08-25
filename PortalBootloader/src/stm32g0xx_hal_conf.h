/* The HAL configuration header, with no HAL module enabled.
 *
 * This file is not vestigial and it is not empty by accident. `framework = stm32cube` compiles
 * every HAL source in the driver, and `stm32g0xx.h` includes `stm32g0xx_hal.h` whenever
 * `USE_HAL_DRIVER` is defined; what decides how much of that survives is which
 * `HAL_*_MODULE_ENABLED` macros are set here. Setting none leaves the HAL sources compiling to
 * empty objects, and the linker discards them.
 *
 * That is worth roughly 6 kB. The v5 bootloader enabled ten modules and used the RCC, UART, DMA
 * and FLASH ones; this build talks to those four peripherals through their registers directly,
 * which is a few hundred bytes of code against the HAL's several thousand, and -- for the flash
 * and receive paths -- has to be, because those routines run from SRAM and the HAL's do not.
 *
 * `board_build.stm32cube.custom_config_header = yes` in `platformio.ini` is what makes the build
 * use this file instead of the framework's own. Without that line it is ignored silently and the
 * framework's version, which enables far more, is used instead.
 *
 * It lives in `src/` rather than `include/` deliberately. The framework puts both directories on
 * the HAL's include path, so either works here -- but `include/` is also on *PortalFW's* include
 * path, because that is where the shared flash-layout header lives, and a file called
 * `stm32g0xx_hal_conf.h` sitting there would be picked up by the Arduino core in preference to its
 * own. The symptom is not subtle (the core's HAL calls stop resolving) but the cause is, so the
 * file is simply kept out of the shared directory.
 */
#ifndef STM32G0xx_HAL_CONF_H
#define STM32G0xx_HAL_CONF_H

#ifdef __cplusplus
extern "C" {
#endif

/* Deliberately no HAL_*_MODULE_ENABLED here.
 *
 * The CMSIS device header still has to arrive, though, and with no module enabled nothing else
 * pulls it in -- `stm32g0xx_hal.h` would then be compiled without so much as `uint32_t` defined.
 * Normally it comes via `stm32g0xx_hal_def.h`, which each enabled module's header includes -- and
 * which also declares `HAL_StatusTypeDef` and friends that `stm32g0xx_hal.h` goes on to use in its
 * own prototypes. Including it directly is that path, made once. The apparent circularity
 * (`stm32g0xx_hal_def.h` includes `stm32g0xx.h`, which includes `stm32g0xx_hal.h`, which includes
 * this file) resolves on the include guards: each defines its own before including the next. */
#include "stm32g0xx_hal_def.h"

/* Oscillator values. Still required: the CMSIS system file and any HAL header that does get
 * included reference them, and they are facts about the board rather than about the HAL. */
#if !defined  (HSE_VALUE)
  #define HSE_VALUE    8000000U   /*!< The crystal fitted to the Portal board. */
#endif

#if !defined  (HSE_STARTUP_TIMEOUT)
  #define HSE_STARTUP_TIMEOUT    100U
#endif

#if !defined  (HSI_VALUE)
  #define HSI_VALUE    16000000U
#endif

#if !defined  (LSI_VALUE)
  #define LSI_VALUE    32000U     /*!< Drives the independent watchdog. */
#endif

#if !defined  (LSE_VALUE)
  #define LSE_VALUE    32768U
#endif

#if !defined  (LSE_STARTUP_TIMEOUT)
  #define LSE_STARTUP_TIMEOUT    5000U
#endif

#define  VDD_VALUE                    3300U
#define  TICK_INT_PRIORITY            0U
#define  USE_RTOS                     0U
#define  PREFETCH_ENABLE              0U
#define  INSTRUCTION_CACHE_ENABLE     0U

/* No run-time parameter checking: it is a debug aid that costs code in an image with a hard size
 * limit, and every value this firmware passes to a peripheral is a constant. */
#define assert_param(expr) ((void)0U)

#ifdef __cplusplus
}
#endif

#endif /* STM32G0xx_HAL_CONF_H */
