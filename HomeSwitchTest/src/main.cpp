// Bench calibration firmware for the optical home switch (Modules::HomeSwitchOptical).
//
// Standalone project (outside PortalFW). It reuses the real HomeSwitchOptical
// class - including its TIM6 software-PWM threshold "DAC" on PC15 - so the
// bench conditions match runtime exactly. See platformio.ini for how the
// shared source is referenced without copying.
//
// What it does, continuously:
//   * Sweeps the shared comparator threshold in ONE direction only (ascending),
//     deliberately ignoring the comparator's hysteresis. For each axis it records
//     the duty at which the comparator output first flips away from its duty-0
//     state - that crossing duty is a proxy for the sensor's analog level.
//   * Tracks a rolling mean / population stdev of the crossing per axis.
//   * Shows live values + a scrolling graph on the OLED (u8g2 graphics).
//   * Streams one machine-parseable CSV line per sweep on the debug UART:
//         D,<millis>,<A_duty|-1>,<B_duty|-1>,<A_sigma_x10>,<B_sigma_x10>,
//           <A_lo>,<A_hi>,<B_lo>,<B_hi>
//     where *_lo/*_hi are the comparator state (1=HIGH) at the bottom/top of the
//     swept range - so the line stays informative even with no crossing (-1).
//     (companion: monitor/home_switch_monitor.py plots this live.)
//
// NOTE on accuracy: the threshold DAC is RC-filtered (tau ~100 ms), so a fast
// ascending sweep reads a duty that is offset HIGH from the true crossing (the
// lagging filter has to be over-driven to catch up). The offset is systematic
// and reproducible for a fixed sweep speed, so it cancels when you take the
// midpoint of two states measured with the same sweep. Calibrate by reading the
// crossing in the "home" and "not-home" target positions and picking a
// HOMESWITCHOPTICAL_DEFAULT_THRESHOLD between them (>> sigma apart).

#include <Arduino.h>
#include <U8g2lib.h>
#include <u8g2hal.h>      // u8x8_stm32_init_i2c + STM32 hw-I2C/GPIO callbacks (u8g2stm32 lib)
#include <math.h>

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

// ---------------------------------------------------------------------------
// Peripherals
// ---------------------------------------------------------------------------
// Same debug UART as runtime (USART1, PB6 TX / PB7 RX @ 115200).
HardwareSerial testSerial(PB7, PB6);

// Own OLED instance (we don't pull in the GUI module). Same panel/wiring as
// runtime: SSD1306 128x64 over hardware I2C2, address 0x3c, rotated U8G2_R2.
static U8G2 u8g2;
static bool oledOk = false;

static Modules::HomeSwitchOptical * homeA = nullptr;
static Modules::HomeSwitchOptical * homeB = nullptr;

// ---------------------------------------------------------------------------
// Sweep / sampling configuration (tune live at the bench)
// ---------------------------------------------------------------------------
static const uint16_t kDutyMin       = 0;
static const uint16_t kDutyMax       = 255;
static const uint32_t kStartSettleMs = 30;    // let the RC filter fall to the window start
static const uint32_t kDwellUs       = 250;   // microseconds per duty step
static const uint16_t kDutyStepFull  = 4;     // coarse step for fast full-range scanning
static const uint16_t kDutyStepFine  = 1;     // fine step once locked onto a crossing
static const int      kWindowMargin  = 25;    // adaptive-window half-width (duty counts)
static const uint32_t kFullSweepEvery = 16;   // force a full-range sweep this often
static const size_t   kStatsWindow   = 16;    // rolling-stats depth (sweeps)
static const uint32_t kOledPeriodMs  = 200;   // OLED redraw throttle (decoupled from sweeps)
static const int      kGraphW        = 128;   // sparkline width (px == history depth)
static const int      kGraphTop      = 34;    // sparkline top y
static const int      kGraphBot      = 63;    // sparkline bottom y

// Rolling window of recent crossing measurements for one axis.
struct EdgeStats {
    uint16_t samples[kStatsWindow];
    size_t   count    = 0;
    size_t   writeIdx = 0;

    void push(uint16_t value) {
        samples[writeIdx] = value;
        writeIdx = (writeIdx + 1) % kStatsWindow;
        if (count < kStatsWindow) count++;
    }
    float mean() const {
        if (count == 0) return 0.0f;
        uint32_t sum = 0;
        for (size_t i = 0; i < count; i++) sum += samples[i];
        return (float) sum / (float) count;
    }
    float stdev() const {
        if (count < 2) return 0.0f;
        float m = mean();
        float acc = 0.0f;
        for (size_t i = 0; i < count; i++) {
            float d = (float) samples[i] - m;
            acc += d * d;
        }
        return sqrtf(acc / (float) count);    // population stdev
    }
};

static EdgeStats statsA;
static EdgeStats statsB;

// Scrolling graph history (newest at the right). -1 == no crossing that sweep.
static int16_t histA[kGraphW];
static int16_t histB[kGraphW];

// Adaptive sweep window state.
static uint16_t winMin = kDutyMin;
static uint16_t winMax = kDutyMax;
static bool     haveWindow = false;
static uint32_t sweepsSinceFull = 0;
static uint32_t sweepCount = 0;

// a/b: first crossing duty per axis, or -1 if none. The *Lo/*Hi flags are the
// comparator state at the bottom and top of the swept range, so the readout
// stays meaningful even when nothing crosses (you can see which rail it sits on).
struct SweepResult {
    int  a;    int  b;
    bool aLo;  bool aHi;
    bool bLo;  bool bHi;
};

// One ascending sweep over [lo, hi] with the given duty step.
static SweepResult doSweep(uint16_t lo, uint16_t hi, uint16_t step) {
    Modules::HomeSwitchOptical::setThreshold((uint8_t) lo);
    delay(kStartSettleMs);

    const bool baseA = homeA->getForwardsActive();
    const bool baseB = homeB->getForwardsActive();

    int aCross = -1;
    int bCross = -1;
    bool lastA = baseA, lastB = baseB;

    for (uint16_t d = lo; ; d += step) {
        if (d > hi) d = hi;                       // always sample the top exactly
        Modules::HomeSwitchOptical::setThreshold((uint8_t) d);
        delayMicroseconds(kDwellUs);

        lastA = homeA->getForwardsActive();
        lastB = homeB->getForwardsActive();
        if (aCross < 0 && lastA != baseA) aCross = (int) d;
        if (bCross < 0 && lastB != baseB) bCross = (int) d;

        if (d >= hi) break;
    }
    return { aCross, bCross, baseA, lastA, baseB, lastB };
}

static void pushHistory(int16_t * hist, int value) {
    memmove(hist, hist + 1, (kGraphW - 1) * sizeof(int16_t));
    hist[kGraphW - 1] = (int16_t) value;
}

// duty 0..255 -> centivolts 0..330 (Vref = 3.3V * duty/255), rounded.
static inline int dutyToCentivolts(int duty) {
    return (duty * 330 + 127) / 255;
}

static int graphY(int duty) {
    int span = kGraphBot - kGraphTop;
    int y = kGraphBot - (duty * span) / 255;
    if (y < kGraphTop) y = kGraphTop;
    if (y > kGraphBot) y = kGraphBot;
    return y;
}

// Live state from the most recent sweep, so we can show something useful even
// when nothing crosses.
static SweepResult lastSweep = { -1, -1, false, false, false, false };

static void formatAxis(char * buf, size_t n, char axis, int duty, int sig,
                       bool stuckHigh) {
    if (duty >= 0) {
        int cv = dutyToCentivolts(duty);
        snprintf(buf, n, "%c%3d %d.%02dV s%d", axis, duty, cv / 100, cv % 100, sig);
    } else {
        // No crossing in range: report the rail it is sitting on.
        snprintf(buf, n, "%c  -- stuck %s", axis, stuckHigh ? "HIGH" : "LOW");
    }
}

static void drawOled() {
    if (!oledOk) return;
    u8g2.clearBuffer();

    char buf[28];

    u8g2.setFont(u8g2_font_5x7_tr);
    u8g2.drawStr(0, 7, "HOME-SW THRESHOLD");
    snprintf(buf, sizeof(buf), "sw%lu", (unsigned long) sweepCount);
    u8g2.drawStr(100, 7, buf);

    u8g2.setFont(u8g2_font_6x12_mr);

    int aDuty = (statsA.count > 0) ? (int) lroundf(statsA.mean()) : -1;
    int bDuty = (statsB.count > 0) ? (int) lroundf(statsB.mean()) : -1;
    int aSig  = (int) lroundf(statsA.stdev());
    int bSig  = (int) lroundf(statsB.stdev());

    // Prefer the live rolling mean; fall back to the stuck rail from last sweep.
    formatAxis(buf, sizeof(buf), 'A', (lastSweep.a >= 0) ? aDuty : -1, aSig, lastSweep.aHi);
    u8g2.drawStr(0, 20, buf);
    formatAxis(buf, sizeof(buf), 'B', (lastSweep.b >= 0) ? bDuty : -1, bSig, lastSweep.bHi);
    u8g2.drawStr(0, 31, buf);

    // Sparkline frame + A (solid) and B (open) traces.
    u8g2.drawHLine(0, kGraphBot + 1 < 64 ? kGraphBot + 1 : 63, kGraphW);
    for (int x = 0; x < kGraphW; x++) {
        if (histA[x] >= 0) u8g2.drawPixel(x, graphY(histA[x]));
        if (histB[x] >= 0) {
            int y = graphY(histB[x]);                 // dotted-ish: every other x
            if (x & 1) u8g2.drawPixel(x, y);
        }
    }
    u8g2.sendBuffer();
}

void setup() {
    SystemClock_Config();

    // Two board LEDs as axis activity indicators (PortalFW Platform.h):
    //   PB3 (LED_INDICATOR) -> axis A comparator HIGH
    //   PB4 (LED_HEARTBEAT) -> axis B comparator HIGH
    pinMode(PB3, OUTPUT);
    pinMode(PB4, OUTPUT);

    testSerial.begin(115200);
    testSerial.println();
    testSerial.println("# HomeSwitchTest - optical home-switch threshold sweep");
    testSerial.println("# format: D,millis,A_duty,B_duty,A_sigma_x10,B_sigma_x10,A_lo,A_hi,B_lo,B_hi");
    testSerial.println("#   duty=-1 means no crossing; *_lo/*_hi are comparator state (1=HIGH) at range ends");

    // OLED bring-up, mirroring Modules::GUI::setup().
    if (u8x8_stm32_init_i2c()) {
        u8g2_Setup_ssd1306_i2c_128x64_noname_f(u8g2.getU8g2(), U8G2_R2,
            u8x8_byte_stm32_hw_i2c, u8x8_stm32_gpio_and_delay);
        u8x8_SetI2CAddress(u8g2.getU8x8(), 0x3c);
        u8g2.begin();
        oledOk = true;
    } else {
        testSerial.println("# WARNING: OLED not detected");
    }

    for (int i = 0; i < kGraphW; i++) { histA[i] = -1; histB[i] = -1; }

    homeA = new Modules::HomeSwitchOptical(Modules::HomeSwitchOptical::Config::A());
    homeB = new Modules::HomeSwitchOptical(Modules::HomeSwitchOptical::Config::B());
    homeA->setup();   // installs the shared TIM6 software-PWM threshold generator
    homeB->setup();
}

void loop() {
    const bool full = !haveWindow || (sweepsSinceFull >= kFullSweepEvery);
    const uint16_t lo = full ? kDutyMin : winMin;
    const uint16_t hi = full ? kDutyMax : winMax;
    const uint16_t step = full ? kDutyStepFull : kDutyStepFine;

    SweepResult res = doSweep(lo, hi, step);
    lastSweep = res;
    sweepCount++;

    // Indicator LEDs: lit when that axis's comparator read HIGH anywhere in the
    // swept range (i.e. the axis is seeing a high value), off when stuck LOW.
    digitalWrite(PB3, (res.aLo || res.aHi) ? HIGH : LOW);
    digitalWrite(PB4, (res.bLo || res.bHi) ? HIGH : LOW);
    sweepsSinceFull = full ? 0 : (sweepsSinceFull + 1);

    if (res.a >= 0) statsA.push((uint16_t) res.a);
    if (res.b >= 0) statsB.push((uint16_t) res.b);
    pushHistory(histA, res.a);
    pushHistory(histB, res.b);

    // Re-center the adaptive window when both axes crossed; otherwise go full.
    if (res.a >= 0 && res.b >= 0) {
        int lo2 = min(res.a, res.b) - kWindowMargin;
        int hi2 = max(res.a, res.b) + kWindowMargin;
        winMin = (uint16_t) constrain(lo2, (int) kDutyMin, (int) kDutyMax);
        winMax = (uint16_t) constrain(hi2, (int) kDutyMin, (int) kDutyMax);
        haveWindow = true;
    } else {
        haveWindow = false;
    }

    // Stream one CSV line per sweep. Trailing fields are the comparator state
    // (1=HIGH/0=LOW) at the bottom and top of the swept range, so a -1 crossing
    // is still informative (which rail each axis is stuck on).
    char line[72];
    snprintf(line, sizeof(line), "D,%lu,%d,%d,%d,%d,%d,%d,%d,%d",
        (unsigned long) millis(), res.a, res.b,
        (int) lroundf(statsA.stdev() * 10.0f),
        (int) lroundf(statsB.stdev() * 10.0f),
        res.aLo ? 1 : 0, res.aHi ? 1 : 0,
        res.bLo ? 1 : 0, res.bHi ? 1 : 0);
    testSerial.println(line);

    // OLED is the slow part (full-frame I2C); refresh it on its own cadence so
    // it doesn't throttle the sweep/serial rate.
    static uint32_t lastOledMs = 0;
    uint32_t now = millis();
    if (now - lastOledMs >= kOledPeriodMs) {
        lastOledMs = now;
        drawOled();
    }
}
