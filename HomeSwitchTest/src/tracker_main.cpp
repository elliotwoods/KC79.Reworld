// Fixed-threshold realtime tracker for the optical home switch.
//
// Companion to the sweep tool (src/main.cpp). Where the sweep tool walks the
// comparator threshold to *find* each axis's crossing duty (slow, ~10 Hz), this
// tool holds the threshold FIXED at the calibrated operating point and just
// reads the two comparator outputs as fast as possible - so you can watch the
// live home-switch state and catch fast transitions as a flag passes the sensor.
//
//   * Board LEDs track the live state instantly (updated every inner-loop pass):
//       PB3 (LED_INDICATOR) -> axis A,  PB4 (LED_HEARTBEAT) -> axis B.
//   * One byte per frame is streamed on the debug UART at up to 60 Hz:
//
//       bit7 = 1   (sync marker, always set)
//       bit0 = A level     bit1 = B level
//       bit2 = A edge      bit3 = B edge   (changed since the last frame)
//       bits 4-6 = 0
//
//     The inner loop runs far faster than 60 Hz, so a brief pulse between two
//     frames is still reported via the edge bits. The PC syncs on bit7; the
//     ASCII boot banner (bytes < 0x80) is ignored by a bit7-sync reader.
//
// Built only by the `home_switch_track` PlatformIO env (entry point: this file).

#include <Arduino.h>

#include "Modules/HomeSwitchOptical.h"

// ---------------------------------------------------------------------------
// Clock config. Duplicated from PortalFW/src/main.cpp because that file (with
// its own setup()/loop()) is excluded from this build. C linkage satisfies the
// framework's weak prototype in SrcWrapper.
// ---------------------------------------------------------------------------
extern "C" void SystemClock_Config(void) {
    RCC_OscInitTypeDef RCC_OscInitStruct = {0};
    RCC_ClkInitTypeDef RCC_ClkInitStruct = {0};

    HAL_PWREx_ControlVoltageScaling(PWR_REGULATOR_VOLTAGE_SCALE1);

    RCC_OscInitStruct.OscillatorType = RCC_OSCILLATORTYPE_HSE;
    RCC_OscInitStruct.HSEState = RCC_HSE_ON;
    RCC_OscInitStruct.PLL.PLLState = RCC_PLL_ON;
    RCC_OscInitStruct.PLL.PLLSource = RCC_PLLSOURCE_HSE;
    RCC_OscInitStruct.PLL.PLLM = RCC_PLLM_DIV1;
    RCC_OscInitStruct.PLL.PLLN = 16;
    RCC_OscInitStruct.PLL.PLLP = RCC_PLLP_DIV2;
    RCC_OscInitStruct.PLL.PLLR = RCC_PLLR_DIV2;
    if (HAL_RCC_OscConfig(&RCC_OscInitStruct) != HAL_OK) {
        Error_Handler();
    }

    RCC_ClkInitStruct.ClockType = RCC_CLOCKTYPE_HCLK | RCC_CLOCKTYPE_SYSCLK | RCC_CLOCKTYPE_PCLK1;
    RCC_ClkInitStruct.SYSCLKSource = RCC_SYSCLKSOURCE_PLLCLK;
    RCC_ClkInitStruct.AHBCLKDivider = RCC_SYSCLK_DIV1;
    RCC_ClkInitStruct.APB1CLKDivider = RCC_HCLK_DIV1;

    if (HAL_RCC_ClockConfig(&RCC_ClkInitStruct, FLASH_LATENCY_2) != HAL_OK) {
        Error_Handler();
    }
}

// Same debug UART as runtime / the sweep tool (USART1, PB6 TX / PB7 RX @ 115200).
HardwareSerial testSerial(PB7, PB6);

static Modules::HomeSwitchOptical * homeA = nullptr;
static Modules::HomeSwitchOptical * homeB = nullptr;

// Fixed operating threshold (duty 0-255). 178 == HOMESWITCHOPTICAL_DEFAULT_THRESHOLD.
static const uint8_t  kThreshold = HOMESWITCHOPTICAL_DEFAULT_THRESHOLD;
// Max serial frame rate to the PC.
static const uint32_t kStreamHz  = 60;
static const uint32_t kFrameUs   = 1000000UL / kStreamHz;   // ~16667 us

void setup() {
    SystemClock_Config();

    // Indicator LEDs (PortalFW Platform.h): PB3 -> A, PB4 -> B.
    pinMode(PB3, OUTPUT);
    pinMode(PB4, OUTPUT);

    testSerial.begin(115200);
    // ASCII banner: bytes < 0x80, so the bit7-sync PC reader skips it.
    testSerial.println();
    testSerial.println("# HomeSwitchTracker - fixed threshold, 1-byte/frame @ <=60Hz");
    testSerial.print("# threshold="); testSerial.println((int) kThreshold);
    testSerial.println("# byte: b7=1 b0=A b1=B b2=Aedge b3=Bedge");

    homeA = new Modules::HomeSwitchOptical(Modules::HomeSwitchOptical::Config::A());
    homeB = new Modules::HomeSwitchOptical(Modules::HomeSwitchOptical::Config::B());
    homeA->setup();   // installs the shared TIM6 software-PWM threshold generator
    homeB->setup();

    Modules::HomeSwitchOptical::setThreshold(kThreshold);   // set once, leave fixed
}

void loop() {
    static bool prevA = false, prevB = false;   // last sampled level
    static bool aEdge = false, bEdge = false;   // sticky edge latch since last frame
    static uint32_t nextFrameUs = 0;

    // Fast inner read - runs far above 60 Hz. LEDs track live; edges are latched.
    const bool a = homeA->getForwardsActive();
    const bool b = homeB->getForwardsActive();

    digitalWrite(PB3, a ? HIGH : LOW);
    digitalWrite(PB4, b ? HIGH : LOW);

    if (a != prevA) aEdge = true;
    if (b != prevB) bEdge = true;
    prevA = a;
    prevB = b;

    // Transmit one framed byte on a strict 60 Hz schedule.
    const uint32_t now = micros();
    if ((int32_t)(now - nextFrameUs) >= 0) {
        nextFrameUs = now + kFrameUs;

        uint8_t frame = 0x80
            | (a ? 0x01 : 0)
            | (b ? 0x02 : 0)
            | (aEdge ? 0x04 : 0)
            | (bEdge ? 0x08 : 0);
        testSerial.write(frame);

        aEdge = false;
        bEdge = false;
    }
}
