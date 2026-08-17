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
#include <U8g2lib.h>
#include <u8g2hal.h>

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

static U8G2 u8g2;
static bool oledOk = false;

static Modules::HomeSwitchOptical * homeA = nullptr;
static Modules::HomeSwitchOptical * homeB = nullptr;

// Fixed operating threshold (duty 0-255).
static const uint8_t  kThreshold = 240;
// Max serial frame rate to the PC.
static const uint32_t kStreamHz  = 60;
static const uint32_t kFrameUs   = 1000000UL / kStreamHz;   // ~16667 us
// OLED refresh rate (slower than the inner loop - I2C is expensive).
static const uint32_t kOledPeriodMs = 100;

static void drawOled(bool a, bool b, uint32_t aEdges, uint32_t bEdges) {
    if (!oledOk) return;
    u8g2.clearBuffer();
    u8g2.setFont(u8g2_font_5x7_tr);
    u8g2.drawStr(0, 7, "HOME-SW TRACKER");

    // Big state indicators: two 50x40 boxes side-by-side, filled when active.
    // A on the left, B on the right.
    const int boxW = 50, boxH = 40, boxY = 12;
    const int aX = 4, bX = 74;

    if (a) {
        u8g2.drawBox(aX, boxY, boxW, boxH);
        u8g2.setDrawColor(0);
    }
    u8g2.drawFrame(aX, boxY, boxW, boxH);
    u8g2.setFont(u8g2_font_10x20_mr);
    u8g2.drawStr(aX + 14, boxY + 26, "A");
    u8g2.setDrawColor(1);

    if (b) {
        u8g2.drawBox(bX, boxY, boxW, boxH);
        u8g2.setDrawColor(0);
    }
    u8g2.drawFrame(bX, boxY, boxW, boxH);
    u8g2.drawStr(bX + 14, boxY + 26, "B");
    u8g2.setDrawColor(1);

    // Edge counters below the boxes.
    char buf[24];
    u8g2.setFont(u8g2_font_5x7_tr);
    snprintf(buf, sizeof(buf), "e%lu", (unsigned long)(aEdges & 0xFFFF));
    u8g2.drawStr(aX + 2, 62, buf);
    snprintf(buf, sizeof(buf), "e%lu", (unsigned long)(bEdges & 0xFFFF));
    u8g2.drawStr(bX + 2, 62, buf);

    u8g2.sendBuffer();
}

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

    if (u8x8_stm32_init_i2c()) {
        u8g2_Setup_ssd1306_i2c_128x64_noname_f(u8g2.getU8g2(), U8G2_R2,
            u8x8_byte_stm32_hw_i2c, u8x8_stm32_gpio_and_delay);
        u8x8_SetI2CAddress(u8g2.getU8x8(), 0x3c);
        u8g2.begin();
        oledOk = true;
    } else {
        testSerial.println("# WARNING: OLED not detected");
    }

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
    static uint32_t aEdgeCount = 0, bEdgeCount = 0;
    static uint32_t lastOledMs = 0;

    // Fast inner read - runs far above 60 Hz. LEDs track live; edges are latched.
    const bool a = homeA->getForwardsActive();
    const bool b = homeB->getForwardsActive();

    digitalWrite(PB3, a ? HIGH : LOW);
    digitalWrite(PB4, b ? HIGH : LOW);

    if (a != prevA) { aEdge = true; aEdgeCount++; }
    if (b != prevB) { bEdge = true; bEdgeCount++; }
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

    // OLED refresh on a slower cadence so I2C doesn't block the inner loop.
    const uint32_t nowMs = millis();
    if (nowMs - lastOledMs >= kOledPeriodMs) {
        lastOledMs = nowMs;
        drawOled(a, b, aEdgeCount, bEdgeCount);
    }
}
