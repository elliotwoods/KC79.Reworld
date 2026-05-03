# STM32G070RBT6 Pin Mapping — PortalFW

**MCU:** STM32G070RBT6 (ARM Cortex-M0+, 128 KB Flash, 36 KB RAM)  
**Package:** LQFP64  
**Board:** Portal PCB (custom)  
**Clock:** 16 MHz PLL (HSI source)

Alternate function numbers (AF0–AF7) are as specified in the STM32G070 datasheet DS12766.

---

## Used Pins

### Indicators

| Pin  | Mode         | Project Use                          | Active Peripheral | Notes                                                         |
|------|--------------|--------------------------------------|-------------------|---------------------------------------------------------------|
| PB3  | OUTPUT       | LED Indicator — motor active         | GPIO              | Also: TIM1_CH2 (AF1), SPI1_SCK (AF0)                         |
| PB4  | OUTPUT (PWM) | LED Heartbeat — slow breath / error  | PWM via TIM3_CH1  | analogWrite maps to TIM3_CH1 (AF1). Also: SPI1_MISO, USART1_CTS (AF4) |

---

### Debug / Logging Serial — USART1

Configured at 115200 baud. Used for human-readable log output and interactive menu (keypress commands).

| Pin  | Mode         | Project Use    | Active Peripheral  | Notes                                                          |
|------|--------------|----------------|--------------------|----------------------------------------------------------------|
| PB6  | USART1_TX    | Debug serial TX | USART1 (AF0)      | Also: I2C1_SCL (AF6), TIM1_CH3 (AF1), TIM16_CH1N (AF2), SPI2_MISO (AF4) |
| PB7  | USART1_RX    | Debug serial RX | USART1 (AF0)      | Also: I2C1_SDA (AF6), TIM17_CH1N (AF2), USART4_CTS (AF4), SPI2_MOSI (AF1) |

---

### RS485 Network — USART2

Configured at 115200 baud with COBS framing and msgpack serialisation. Half-duplex bus shared across all Portal units. DE pin controls transmit/receive direction.

| Pin  | Mode         | Project Use         | Active Peripheral  | Notes                                                        |
|------|--------------|---------------------|--------------------|--------------------------------------------------------------|
| PA1  | OUTPUT       | RS485 DE (Driver Enable) | GPIO          | HIGH = transmit, LOW = receive. Also: TIM15_CH1N (AF5), USART4_RX (AF4), USART2_RTS (AF1), SPI1_SCK (AF0), ADC1_IN1 |
| PA2  | USART2_TX    | RS485 TX            | USART2 (AF1)       | Also: SPI1_MOSI (AF0), TIM15_CH1 (AF5), ADC1_IN2            |
| PA3  | USART2_RX    | RS485 RX            | USART2 (AF1)       | Also: SPI2_MISO (AF0), TIM15_CH2 (AF5), ADC1_IN3            |

---

### ID Daisy-Chain Serial — USART3

Configured at 115200 baud. Used to assign sequential IDs along a daisy-chain of Portal units. Each unit increments the received ID by 1 and forwards it to the next.

| Pin  | Mode         | Project Use     | Active Peripheral  | Notes                                                        |
|------|--------------|-----------------|-------------------|--------------------------------------------------------------|
| PB8  | USART3_TX    | ID chain TX     | USART3 (AF4)       | Also: TIM16_CH1 (AF2), I2C1_SCL (AF6), SPI2_SCK (AF1)       |
| PB9  | USART3_RX    | ID chain RX     | USART3 (AF4)       | Also: TIM17_CH1 (AF2), I2C1_SDA (AF6), SPI2_NSS (AF5)       |

---

### Board ID DIP Switches — 4-bit binary input

Four GPIO pins read as a 4-bit active-low binary number (INPUT_PULLUP). The result is incremented by 1 so that the all-zeros state maps to ID 1 (ID 0 is reserved for the host). Gives IDs 1–16.

| Pin  | Mode          | Project Use     | Active Peripheral | Notes                                               |
|------|---------------|-----------------|-------------------|-----------------------------------------------------|
| PD0  | INPUT_PULLUP  | Board ID bit 0  | GPIO              | Also: TIM16_CH1 (AF2), SPI2_NSS (AF1)              |
| PD1  | INPUT_PULLUP  | Board ID bit 1  | GPIO              | Also: TIM17_CH1 (AF2), SPI2_SCK (AF1)              |
| PD2  | INPUT_PULLUP  | Board ID bit 2  | GPIO              | Also: TIM1_CH1N (AF2), USART3_RTS (AF0)            |
| PD3  | INPUT_PULLUP  | Board ID bit 3  | GPIO              | Also: TIM1_CH2N (AF2), USART2_CTS (AF0), SPI2_MISO (AF1) |

---

### Motor Driver — Axis A

Step pulse uses TIM16 in PWM mode (PA_6_ALT1 = TIM16_CH1, AF5). Fault is an open-drain active-low input from the DRV8825-compatible driver IC.

| Pin  | Mode          | Project Use        | Active Peripheral    | Notes                                                          |
|------|---------------|--------------------|----------------------|----------------------------------------------------------------|
| PA4  | INPUT         | Motor A Fault      | GPIO                 | Active low. Also: SPI1_NSS (AF0), SPI2_MOSI (AF1), TIM14_CH1 (AF4), ADC1_IN4 |
| PA5  | OUTPUT        | Motor A Enable     | GPIO                 | HIGH = enabled. Also: SPI1_SCK (AF0), USART3_TX (AF4), ADC1_IN5 |
| PA6  | TIM16_CH1 PWM | Motor A Step pulse | TIM16 (AF5 alt)      | Uses PA_6_ALT1 to select TIM16. Also available: TIM3_CH1 (AF1), SPI1_MISO (AF0), USART3_CTS (AF4), ADC1_IN6 |
| PA7  | OUTPUT        | Motor A Direction  | GPIO                 | Also: TIM1_CH1N (AF2), TIM3_CH2 (AF1), TIM14_CH1 (AF4), TIM17_CH1 (AF5), SPI1_MOSI (AF0), ADC1_IN7 |

---

### Motor Driver — Axis B

Step pulse uses TIM1 in PWM mode (PA_10 = TIM1_CH3, AF2).

| Pin  | Mode          | Project Use        | Active Peripheral    | Notes                                                          |
|------|---------------|--------------------|----------------------|----------------------------------------------------------------|
| PA8  | INPUT         | Motor B Fault      | GPIO                 | Active low. Also: TIM1_CH1 (AF2), SPI2_NSS (AF1)              |
| PA9  | OUTPUT        | Motor B Enable     | GPIO                 | HIGH = enabled. Also: TIM1_CH2 (AF2), USART1_TX (AF1), I2C1_SCL (AF6), SPI2_MISO (AF4) |
| PA10 | TIM1_CH3 PWM  | Motor B Step pulse | TIM1 (AF2)           | Also: USART1_RX (AF1), I2C1_SDA (AF6), SPI2_MOSI (AF0)        |
| PA11 | OUTPUT        | Motor B Direction  | GPIO                 | Also: TIM1_CH4 (AF2), USART1_CTS (AF1), I2C2_SCL (AF6), SPI1_MISO (AF0) |

---

### Motor Driver Shared Settings

Both axes share a single DRV8825-compatible driver IC for sleep, microstep resolution, and current limit. M0/M1 are driven in three states: HIGH, LOW, or Hi-Z (input mode) to encode 8 microstepping levels (1/1 to 1/256). VREF uses PWM + external resistor divider (10 kΩ / (22 kΩ + 10 kΩ)) to set the current reference voltage.

| Pin  | Mode              | Project Use                    | Active Peripheral | Notes                                                           |
|------|-------------------|--------------------------------|-------------------|-----------------------------------------------------------------|
| PB0  | OUTPUT            | Motor Driver Sleep             | GPIO              | LOW = sleep. Also: TIM1_CH2N (AF2), TIM3_CH3 (AF1), USART3_RX (AF4), SPI1_NSS (AF0), ADC1_IN8 |
| PB1  | OUTPUT or INPUT   | Motor Driver M0 (microstep)    | GPIO (tri-state)  | Set HIGH, LOW, or Hi-Z to encode resolution. Also: TIM1_CH3N (AF2), TIM3_CH4 (AF1), TIM14_CH1 (AF0), USART3_RTS (AF4), ADC1_IN9 |
| PB2  | OUTPUT or INPUT   | Motor Driver M1 (microstep)    | GPIO (tri-state)  | Set HIGH, LOW, or Hi-Z to encode resolution. Also: USART3_TX (AF4), SPI2_MISO (AF1), ADC1_IN10 |
| PB15 | OUTPUT (PWM)      | Motor Driver VREF (current)    | PWM via TIM       | analogWrite sets duty cycle → voltage → motor current. Also: TIM1_CH3N (AF2), TIM15_CH1N (AF4), TIM15_CH2 (AF5), SPI2_MOSI (AF0) |

---

### Home / Limit Switches

Active-low inputs (switch pulls to GND). No pull-up resistors configured in code (bare INPUT mode — relies on external pull-ups or driver IC internal pull-ups on the connector).

| Pin  | Mode   | Project Use             | Active Peripheral | Notes                                                |
|------|--------|-------------------------|-------------------|------------------------------------------------------|
| PC7  | INPUT  | Home Switch A Backwards | GPIO              | Also: TIM3_CH2 (AF1)                                 |
| PC13 | INPUT  | Home Switch A Forwards  | GPIO              | Limited alternate functions on PC13 on this MCU.                    |
| PC14 | INPUT  | Home Switch B Backwards | GPIO              | OSC32_IN — using this disables any LSE/RTC crystal   |
| PC15 | INPUT  | Home Switch B Forwards  | GPIO              | OSC32_OUT — using this disables any LSE/RTC crystal  |

---

### OLED Display — I2C2

SSD1306 128×64 OLED at address 0x3C. Uses Arduino Wire library routed through I2C2. The u8g2 library is configured with software callbacks (`U8X8_NO_HW_I2C` build flag disables u8g2's own I2C driver; Wire is used instead). Display initialisation is optional — if no device is found, the GUI is silently disabled.

| Pin  | Mode      | Project Use      | Active Peripheral | Notes                                                       |
|------|-----------|------------------|-------------------|-------------------------------------------------------------|
| PB10 | I2C2_SCL  | OLED display SCL | I2C2 (AF6)        | Also: USART3_TX (AF4), SPI2_SCK (AF5), ADC1_IN11            |
| PB11 | I2C2_SDA  | OLED display SDA | I2C2 (AF6)        | Also: USART3_RX (AF4), SPI2_MOSI (AF0), ADC1_IN15           |

---

### Debug / Programming Interface — SWD

Standard STM32 two-wire debug interface. Always active at reset; cannot be repurposed while a debugger is attached.

| Pin  | Mode   | Project Use             | Active Peripheral | Notes                                   |
|------|--------|-------------------------|-------------------|-----------------------------------------|
| PA13 | SWD    | SWDIO (data I/O)        | SWD (AF0)         | Can be GPIO if SWD is released in firmware |
| PA14 | SWD    | SWCLK (clock)           | SWD (AF0)         | Also: USART2_TX (AF1) if SWD released  |

---

## Unused Pins

Pins not referenced anywhere in the project source. All can be used as GPIO at minimum; alternate functions listed below each.

| Pin  | Available Functions                                                                 |
|------|-------------------------------------------------------------------------------------|
| PA0  | ADC1_IN0, USART4_TX (AF4), USART2_CTS (AF1), SPI2_SCK (AF0)                       |
| PA12 | I2C2_SDA (AF6), USART1_RTS (AF1), SPI1_MOSI (AF0)                                 |
| PA15 | SPI1_NSS (AF0), USART2_RX (AF1), USART3_RTS (AF5), USART4_RTS (AF4)              |
| PB5  | SPI1_MOSI (AF0), TIM3_CH2 (AF1)                                                    |
| PB12 | SPI2_NSS (AF0), ADC1_IN16                                                          |
| PB13 | SPI2_SCK (AF0), TIM1_CH1N (AF2), TIM15_CH1N (AF5), USART3_CTS (AF4), I2C2_SCL (AF6) |
| PB14 | SPI2_MISO (AF0), TIM1_CH2N (AF2), TIM15_CH1 (AF5), USART3_RTS (AF4), I2C2_SDA (AF6) |
| PC0  | GPIO only                                                                           |
| PC1  | TIM15_CH1 (AF2), GPIO                                                              |
| PC2  | TIM15_CH2 (AF2), SPI2_MISO (AF1), GPIO                                            |
| PC3  | SPI2_MOSI (AF1), GPIO                                                              |
| PC4  | ADC1_IN17, USART1_TX (AF1), USART3_TX (AF0)                                       |
| PC5  | ADC1_IN18, USART1_RX (AF1), USART3_RX (AF0)                                       |
| PC6  | TIM3_CH1 (AF1), GPIO                                                               |
| PC8  | TIM1_CH1 (AF2), TIM3_CH3 (AF1), GPIO                                              |
| PC9  | TIM1_CH2 (AF2), TIM3_CH4 (AF1), GPIO                                              |
| PC10 | TIM1_CH3 (AF2), USART3_TX (AF0), USART4_TX (AF1)                                  |
| PC11 | TIM1_CH4 (AF2), USART3_RX (AF0), USART4_RX (AF1)                                  |
| PC12 | TIM14_CH1 (AF2), GPIO                                                              |
| PD4  | TIM1_CH3N (AF2), USART2_RTS (AF0), SPI2_MOSI (AF1)                                |
| PD5  | USART2_TX (AF0), SPI1_MISO (AF1)                                                   |
| PD6  | USART2_RX (AF0), SPI1_MOSI (AF1)                                                   |
| PF0  | TIM14_CH1 (AF2), OSC_IN (external HSE crystal)                                     |
| PF1  | TIM15_CH1N (AF2), OSC_OUT (external HSE crystal)                                   |
| PF2  | MCO clock output (AF0), NRST filter                                                 |

---

## Notes

- **Four USARTs available; all four in use:** USART1 (debug), USART2 (RS485), USART3 (ID chain), USART4 unused.
- **Both I2C peripherals:** I2C1 pins (PB6/PB7, PB8/PB9, PA9/PA10) overlap with USART1 and USART3. I2C2 is used for the display on PB10/PB11; a second I2C2 option (PB13/PB14) remains free.
- **PC14/PC15** are nominally the 32 kHz crystal pins (OSC32). Using them as GPIO means no low-speed external oscillator (no RTC). The G070 can run RTC from LSI instead.
- **PB15 VREF** uses `analogWrite` which the STM32 Arduino core maps to hardware PWM. The exact timer channel assigned depends on the core's pin-timer table; TIM1_CH3N, TIM15_CH1N, or TIM15_CH2 are the candidates on PB15.
- **PA6 Step A** uses `PA_6_ALT1` (`PinName` enum) to force selection of TIM16_CH1 (AF5) over the default TIM3_CH1 (AF1), since TIM1 is already claimed by Motor B step.
- **SWD pins PA13/PA14** can be released to GPIO in firmware, but doing so before re-flashing locks out the debugger until the next power cycle with BOOT0 asserted.
