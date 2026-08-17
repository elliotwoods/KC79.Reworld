// Unified Side-A bench firmware for the optical home switch + ring-gear homing.
//
// One firmware that the cross-platform GUI (monitor/home_switch_gui.py) drives
// over the ST-Link debug UART (USART1, PB6 TX / PB7 RX @ 115200). It reuses the
// real PortalFW driver classes (HomeSwitchOptical, MotorDriver,
// MotorDriverSettings) and a lightweight self-contained motion controller
// (BenchMotion) so we can:
//   * read the Side-A optical sensor live at a fixed (settable) threshold,
//   * sweep the comparator threshold to find the crossing duty (calibration),
//   * jog / go-to the ring gear with the Axis-A stepper,
//   * run a two-edge homing sequence and report the result.
//
// Bidirectional line protocol (each message is one '\n'-terminated ASCII line;
// it is also fine to type by hand in a serial terminal):
//
//   Host -> firmware
//     T <duty>            set comparator threshold (0..255)
//     M <0|1|2>           mode: 0 idle, 1 track (live sensor), 2 sweep
//     E <0|1>             motor coils off / on (holding torque)
//     J <dMicrosteps> [v] jog relative (signed), optional speed (usteps/s)
//     G <microsteps> [v]  go to absolute position
//     V <v>               set default speed (usteps/s)
//     H                   run homing sequence (Side A)
//     Z                   zero current position
//     X                   abort current motion / routine
//     P                   emit one status line now
//     R <microcode> <mA>  set microstep resolution code + coil current
//     C <signedSpeed>     continuous jog (non-blocking); 0 stops
//     Q                   one threshold sweep at the current position
//     F <N>               coarse-to-fine peak search over +/-N microsteps
//     K <N> [step] [dutyMin] [dutyMax]  home-shape grid scan (dutyMin<lowest crossing, ~200)
//     A <N> [step]        self-calibrating two-edge dip home (finds its own threshold)
//     O [vEdge] [M] [vSeek] [accel] [forceCal] [passes]  FAST home + backlash (self-cal T)
//     N [T] [vmax] [accel] [M]        full-rev sensor census at a fixed threshold
//     Y <dMicrosteps> [vmax] [accel]  ramped (trapezoid) relative move - speed probing
//
//   Firmware -> host
//     S,<ms>,<level>,<thr>,<pos>,<deg_x10>,<running>,<enabled>,<fault>,<homed>
//     D,<ms>,<A_duty>,<B_duty>,<A_sigma_x10>,<B_sigma_x10>,<A_lo>,<A_hi>,<B_lo>,<B_hi>
//     H,<ok>,<home>,<switchSize>,<leadingEdge>,<trailingEdge>,"<message>"
//     Q,<pos>,<cross>[,<lo>,<hi>]  one crossing sweep (Q cmd) / peak-search sample
//     B,<bestPos>,<bestCross>,<threshold>   peak-search result
//     K,begin,<center>,<n>,<step>,<count>,<dutyMin>,<dutyMax>   home-shape scan start
//     K,<index>,<pos>,<cross>,<lo>,<hi>     home-shape scan sample (cross -1 = none)
//     K,end,<center>,<samples>,<aborted>    home-shape scan end
//     A,begin,<center>,<n>,<step>,<dutyMin>,<dutyMax>   auto-home start
//     A,pt,<index>,<pos>,<cross>,<lo>,<hi>  auto-home coarse characterisation point
//     A,thr,<T>,<cmin>,<pmin>,<dipDepth>    chosen threshold into the dip
//     A,edge,<0|1>,<pos>                    leading (0) / trailing (1) edge found
//     A,done,<ok>,<home>,<lead>,<trail>,<switch>,<T>,<dipDepth>,"<msg>"   auto-home result
//     O,begin,<pos>,<T>,<vEdge>,<M>,<vSeek>   fast-home start (T=0 -> will calibrate)
//     O,cal,<c1>,<c2>,<bg>,<T>                background probe -> chosen threshold
//     O,thr,<T>,up|down                       adaptive threshold bump mid-routine
//     O,seek,<coarseLead>                     coarse leading edge from the seek
//     O,gate,<name>,<pass>,<info>             validation gate (depth / shoulder / width)
//     O,edge,<0|1>,<pos>                      precise leading / trailing edge
//     O,pass,<i>,<lead>,<trail>               one completed precise pass
//     O,backlash,<microsteps>,<reenterPos>    backlash measurement
//     O,done,<ok>,<home>,<lead>,<trail>,<switch>,<backlash>,<T>,<ms>,"<msg>"
//     N,begin,<pos>,<T>,<vmax>,<lap>          census start
//     N,edge,<i>,<pos>,<state>                census transition (state = new level)
//     N,end,<count>,<aborted>                 census end
//     L,<level>,<message>          progress / log
//     #...                         human-readable banner / comments

#include <Arduino.h>
#include <U8g2lib.h>
#include <u8g2hal.h>
#include <math.h>
#include <stdlib.h>
#include <string.h>

#include "Modules/HomeSwitchOptical.h"
#include "Modules/MotorDriver.h"
#include "Modules/MotorDriverSettings.h"
#include "BenchMotion.h"

// ---------------------------------------------------------------------------
// Clock config (copied from PortalFW/src/main.cpp; that file is excluded here).
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
// Peripherals / globals
// ---------------------------------------------------------------------------
HardwareSerial testSerial(PB7, PB6);   // USART1, same as runtime

static U8G2 u8g2;
static bool oledOk = false;

static Modules::HomeSwitchOptical * sensorA = nullptr;
static Modules::MotorDriverSettings * motorSettings = nullptr;
static Modules::MotorDriver * motorDriverA = nullptr;
static Bench::Motion * motion = nullptr;

enum Mode : uint8_t { MODE_IDLE = 0, MODE_TRACK = 1, MODE_SWEEP = 2 };
static Mode mode = MODE_TRACK;

static uint8_t threshold = HOMESWITCHOPTICAL_DEFAULT_THRESHOLD;
static bool    busy = false;     // true while a blocking motion/routine runs
static bool    homed = false;

static const uint32_t kStatusPeriodMs = 16;    // ~60 Hz status stream
static const uint32_t kOledPeriodMs   = 100;   // 10 Hz OLED

// ---------------------------------------------------------------------------
// Sweep state (Side A only; B fields reported as absent)
// ---------------------------------------------------------------------------
static const uint16_t kSweepStep     = 2;
static const uint32_t kSweepSettleMs = 30;
static const uint32_t kSweepDwellUs  = 250;
static const size_t   kSweepWindow   = 16;
static const uint32_t kSettleMs      = 250;   // RC-filter settle per step (~2.5 tau)

// Home-shape grid scan (command K) tunables.
static const int      kScanDutyStep = 4;      // duty increment per ascending sweep step
static const uint32_t kScanSettleMs = 200;    // drain RC to the duty-0 branch (~2 tau) per position
static const uint32_t kScanDwellMs  = 20;     // per-duty-step serviced dwell (> PWM ripple ~12.5 ms)
static const Steps    kScanBacklash = 2000;   // forward take-up before the sweep starts
static const int      kScanDutyMin  = 200;    // no real crossing occurs below ~200 (bench physics)

// Self-calibrating dip-home (command A) tunables.
static const float    kAutoThreshFrac       = 0.6f;   // T = cmin + frac*(255-cmin): upper-middle of dip
static const int      kAutoMinDipDepth      = 8;      // reject noise-only features (255 - cmin must exceed)
static const uint32_t kAutoThresholdSettleMs = 300;   // ~3 tau after setThreshold(T) before the edge pass
static const uint32_t kAutoHomeTimeoutMs    = 120000; // overall edge-pass budget (same as H)
static const int      kAutoMaxPoints        = 96;     // coarse characterisation buffer
static Steps  autoPos[kAutoMaxPoints];
static int    autoCross[kAutoMaxPoints];

// Fast home + backlash (command O) tunables. Threshold policy is
// BACKGROUND-RELATIVE: the whole reflection profile drifts day to day (Jul-8:
// shoulders 252 / floor 236; Jul-9: shoulders 252 / floor <200; Jul-10:
// background ~240 / floor ~218 and NOTHING dark at 246), so no fixed duty
// works. Instead the routine measures the LOCAL BACKGROUND crossing at run
// time (two settled probes 3000 usteps apart - at most one can sit on the
// dip) and anchors T_op = background - margin: background reads INACTIVE,
// only the deep dip reads ACTIVE. The dip floor is only ever a go/no-go
// depth gate and is never used to derive the threshold.
static const int      kFastBgMargin      = 10;     // T_op = background crossing - this
static const int      kFastDepthBelowBg  = 6;      // dip probe must read <= anchorBg - this
                                                   // (dip floor can sit only ~4-8 below T_op on
                                                   // shallow days; background reads T_op + margin)
static const int      kFastCalBracketLo  = 140;    // settled-probe search bracket [lo, 255]
static const int      kFastCalIters      = 5;      // binary-search probes (resolution ~4 duty)
static const uint32_t kFastCalSettleMs   = 220;    // RC settle per probe (~2.2 tau)
static const uint32_t kFastSettleMs      = 300;    // RC settle after a large DAC change (~3 tau)
static const uint32_t kFastGateSettleMs  = 250;    // RC settle for small DAC steps (~2.5 tau)
static const int      kFastTAdjustDown   = 8;      // cannot find inactive background -> T down
static const int      kFastTAdjustUp     = 6;      // seek found nothing in a lap -> T up
static const int      kFastMaxTAdjust    = 4;      // adaptive bumps before giving up
static const int      kFastPassesDefault          = 2;       // precise passes averaged per home
static const Steps    kFastPassDisagree           = 12;      // 2 passes apart by more than this
                                                             // -> take a 3rd, use the median
static const uint32_t kFastTimeoutMs     = 60000;
static const int      kFastMaxRejects    = 3;      // false features skipped before giving up

// Per-motor parameter sets, named by output gear ratio. Two module
// generations exist: the original 32:1 and the 2x-faster 16:1 (same physical
// flag/backlash geometry, so every microstep-denominated LENGTH halves; the
// SPEEDS are motor-side pulse rates, carried over as starting points and then
// re-tuned on the bench per motor). The active set is selected by fast home's
// motor detection (or forced with the U command).
struct MotorParams {
    uint8_t                 ratio;          // output gear ratio (set name)
    StepsPerSecond          seekSpeed;      // ramped-seek cruise (forward; slip here
                                            // is harmless - the seek is coarse)
    StepsPerSecondPerSecond seekAccel;      // ramp acceleration
    StepsPerSecond          reposSpeed;     // datum-critical approach moves (the
                                            // backward re-approaches between passes:
                                            // slip THERE biases the datum)
    StepsPerSecond          edgeSpeed;      // v_edge for precise passes
    uint16_t                debounceM;      // latch debounce M
    Steps                   clearance;      // back-off before an armed forward pass (> backlash)
    Steps                   takeup;         // overshoot-return so approaches are forward-engaged
    Steps                   widthMin;       // plausible flag width at T_op (usteps)
    Steps                   widthMax;
    Steps                   halfWidthGuess; // centre estimate = coarseLead + this
    Steps                   backlashWalk;   // 32:1: == production debounceDistance (32 fullsteps x 32)
    Steps                   backlashMax;    // backlash outside [-32, max] fails the routine
};
// 32:1 speeds: the motor STALLS above ~30-32k usteps/s when ramped at
// 100k/s^2 (measured on this rig; it can cruise 36k+ only with accel
// <=20k/s^2, which wins no time). 24k = 20% below the stall cliff. v_edge
// 2000 + M32: edge loci repeat to ~1 ustep within a run (knee data); flank
// dither blips are ~10-35 usteps.
static const MotorParams kParams32 = { 32, 24000, 100000, 24000, 2000, 32,
                                       5000, 2000, 700, 4200, 900, 1024, 3000 };
// 16:1, bench-tuned Jul 20-21 2026: forward stall cliff 17-19k @ 100k/s^2
// (halved gearing doubles the reflected torque) -> seek 14k. BACKWARD has a
// mid-band resonance that stalls the motor DEAD at a 5-6k cruise when cold
// (and swallows 10-14k when hot) while 4k and 7-10k run clean - so all
// datum-critical approach moves run at 4k, below any band, where torque is
// also maximal. The new sensor's dip is ~24 counts deep (vs 10-14 on 32:1)
// with steep flanks: vEdge 2000 + M48 measured sd 2.1 usteps / span 6 over 8
// stay-at-park homes - as repeatable as vEdge 500 and 3x faster; 3000 is
// past the knee. Flag width ~830 usteps at T_op; backlash 144-476
// (motor-gearbox dominated).
static const MotorParams kParams16 = { 16, 14000, 100000, 4000, 2000, 48,
                                       2500, 1000, 350, 2100, 450, 512, 1500 };

// Active set + whether the gear ratio has been confirmed (by detection or a
// U override) since power-up. Unconfirmed runs use 32:1 budgets - the larger
// of the two, safe for both motors.
static const MotorParams * mp = &kParams32;
static bool motorConfirmed = false;

// Announce the geometry so the host can convert microsteps <-> degrees.
// Printed at boot and re-printed whenever the gear ratio changes (motor
// detection / U command) - the host tools adopt any "# ... usteps_per_rev="
// line, so a mid-session change needs no extra host-side parsing.
static void emitBanner() {
    char buf[80];
    snprintf(buf, sizeof(buf), "# usteps_per_rev=%ld default_threshold=%d gear_ratio=%d",
        (long) motion->getMicrostepsPerPrismRotation(), (int) threshold,
        (int) motion->getGearRatio());
    testSerial.println(buf);
}

// Calibrated operating threshold, cached across runs (0 = not calibrated).
// Warm runs skip the ~3.5 s calibration; any routine failure clears it.
static int fastTCached = 0;

// Width servo: slow thermal drift moves the whole crossing profile relative
// to a fixed T, walking the operating point along the (asymmetric) flanks -
// the dominant repeatability limit once measurement noise is averaged out.
// Width is a free, high-gain drift signal (~38 usteps per duty count), so a
// +/-1 count trim of the cached T whenever the measured width strays from
// the anchor (captured at calibration) re-pins the operating point.
static Steps fastWidthAnchor = 0;
static const Steps kFastWidthServoBand = 45;   // ~1.2 counts of width drift

// Census (command N) buffers: one full-rev lap of debounced transitions.
static Steps   censusEdges[128];
static uint8_t censusEdgeStates[128];

static uint16_t sweepSamples[kSweepWindow];
static size_t   sweepCount = 0, sweepWrite = 0;

static void sweepPush(uint16_t v) {
    sweepSamples[sweepWrite] = v;
    sweepWrite = (sweepWrite + 1) % kSweepWindow;
    if (sweepCount < kSweepWindow) sweepCount++;
}
static float sweepStdev() {
    if (sweepCount < 2) return 0.0f;
    float m = 0; for (size_t i = 0; i < sweepCount; i++) m += sweepSamples[i];
    m /= (float) sweepCount;
    float acc = 0; for (size_t i = 0; i < sweepCount; i++) {
        float d = (float) sweepSamples[i] - m; acc += d * d;
    }
    return sqrtf(acc / (float) sweepCount);
}

// ---------------------------------------------------------------------------
// Telemetry
// ---------------------------------------------------------------------------
static void emitStatus() {
    Steps pos = motion->getPosition();
    long degx10 = (long) lroundf(3600.0f * (float) pos
        / (float) motion->getMicrostepsPerPrismRotation());
    char line[96];
    snprintf(line, sizeof(line), "S,%lu,%d,%d,%ld,%ld,%d,%d,%d,%d",
        (unsigned long) millis(),
        motion->sensorActive() ? 1 : 0,
        (int) threshold,
        (long) pos,
        degx10,
        motion->getRunning() ? 1 : 0,
        motion->getEnabled() ? 1 : 0,
        motion->getFault() ? 1 : 0,
        homed ? 1 : 0);
    testSerial.println(line);
}

static uint32_t lastStatusMs = 0;
static void emitStatusThrottled() {
    uint32_t now = millis();
    if (now - lastStatusMs >= kStatusPeriodMs) {
        lastStatusMs = now;
        emitStatus();
    }
}

static void emitLog(int level, const char * msg) {
    char line[96];
    snprintf(line, sizeof(line), "L,%d,%s", level, msg);
    testSerial.println(line);
}

// ---------------------------------------------------------------------------
// Idle driver sleep: no holding torque is required while static, and holding
// current is the dominant heat source on the small 16:1 motor (measured:
// sustained duty collapses its stall envelope within ~20 min). After
// kIdleSleepMs of no motion (and not mid-routine) the driver is put to sleep
// (nSLEEP low = outputs off); any motion command wakes it first. NB sleep
// resets the driver's microstep indexer, so the rotor can snap to a nearby
// phase position on wake - fine between operations (the next home
// re-references everything), never used inside a routine (busy holds it off).
// ---------------------------------------------------------------------------
static const uint32_t kIdleSleepMs = 1000;
static bool     driverAsleep = false;
static uint32_t lastActiveMs = 0;

static void driverWake() {
    if (driverAsleep) {
        motorSettings->setSleep(false);
        delay(2);                        // tWAKE ~1 ms; give margin
        driverAsleep = false;
    }
    lastActiveMs = millis();
}

// ---------------------------------------------------------------------------
// Command parser
// ---------------------------------------------------------------------------
static void ensureMovableMode() {
    // Sweeping owns the threshold DAC, so leave sweep mode and restore the
    // fixed operating threshold before any motion.
    if (mode == MODE_SWEEP) {
        mode = MODE_TRACK;
        Modules::HomeSwitchOptical::setThreshold(threshold);
    }
    driverWake();
}

// Defined below (used by the command handler).
static int measureCrossingA(int& loOut, int& hiOut);
static int measureCrossingSettled(int& loOut, int& hiOut);
static void searchPeak(Steps N);
static bool waitServiced(uint32_t ms);
static int measureCrossingAscending(int dutyMin, int dutyMax, int dutyStep,
                                    uint32_t settleMs, uint32_t dwellMs,
                                    int& loOut, int& hiOut);
static void gridScan(Steps n, Steps step, int dutyMin, int dutyMax);
static void autoHomeFail(const char * msg, Steps parkTo);
static void autoHome(Steps n, Steps step);
static void fastHome(StepsPerSecond vEdge, int m, StepsPerSecond vSeek,
                     StepsPerSecondPerSecond accel, bool forceCal, int passes);
static void censusRun(int T, StepsPerSecond vmax, StepsPerSecondPerSecond accel,
                      uint16_t m);

static void handleCommand(char * line) {
    // Trim leading spaces.
    while (*line == ' ' || *line == '\t') line++;
    if (*line == 0 || *line == '#') return;

    char cmd = *line;
    char * args = line + 1;

    // While a blocking motion is running, only abort / threshold / status are
    // honoured (prevents re-entrancy from the service callback). Announce the
    // drop so a pipelining host can't silently deadlock waiting for a reply.
    if (busy && cmd != 'X' && cmd != 'T' && cmd != 'P') {
        char msg[24];
        snprintf(msg, sizeof(msg), "busy drop %c", cmd);
        emitLog(1, msg);
        return;
    }

    switch (cmd) {
        case 'T': {
            int v = atoi(args);
            if (v < 0) v = 0; if (v > 255) v = 255;
            threshold = (uint8_t) v;
            Modules::HomeSwitchOptical::setThreshold(threshold);
            break;
        }
        case 'M': {
            int m = atoi(args);
            mode = (Mode) m;
            if (mode == MODE_TRACK || mode == MODE_IDLE) {
                Modules::HomeSwitchOptical::setThreshold(threshold);
            }
            sweepCount = 0; sweepWrite = 0;
            break;
        }
        case 'E': {
            if (atoi(args) != 0) driverWake();
            motion->enable(atoi(args) != 0);
            break;
        }
        case 'V': {
            motion->setDefaultSpeed((StepsPerSecond) atol(args));
            break;
        }
        case 'Z': {
            motion->zero();
            emitLog(0, "zeroed");
            break;
        }
        case 'X': {
            motion->requestAbort();               // aborts a running blocking routine
            if (!busy) motion->runContinuous(0);   // ...or halts a continuous jog
            break;
        }
        case 'C': {                                // continuous jog (non-blocking)
            ensureMovableMode();
            motion->runContinuous((StepsPerSecond) atol(args));
            break;
        }
        case 'Q': {                                // one sweep at the current position
            ensureMovableMode();
            int lo, hi;
            int cross = measureCrossingSettled(lo, hi);
            char line[64];
            snprintf(line, sizeof(line), "Q,%ld,%d,%d,%d",
                (long) motion->getPosition(), cross, lo, hi);
            testSerial.println(line);
            Modules::HomeSwitchOptical::setThreshold(threshold);   // restore the DAC
            break;
        }
        case 'F': {                                // coarse-to-fine peak search
            ensureMovableMode();
            Steps N = (Steps) atol(args);
            if (N <= 0) N = 500;
            busy = true;
            motion->clearAbort();
            searchPeak(N);
            busy = false;
            emitStatus();
            break;
        }
        case 'K': {                                // whole-curve home-shape scan
            ensureMovableMode();
            char * tok  = strtok(args, " \t");
            Steps n     = tok  ? (Steps) atol(tok)  : 0;
            char * tok2 = strtok(nullptr, " \t");
            Steps step  = tok2 ? (Steps) atol(tok2) : 0;
            char * tok3 = strtok(nullptr, " \t");
            int dutyMin = tok3 ? atoi(tok3) : kScanDutyMin;   // default 200
            char * tok4 = strtok(nullptr, " \t");
            int dutyMax = tok4 ? atoi(tok4) : 255;            // default 255
            if (n    <= 0) n    = 500;
            if (step <= 0) step = 10;
            if (dutyMin < 0) dutyMin = 0; if (dutyMin > 255) dutyMin = 255;
            if (dutyMax < 0) dutyMax = 0; if (dutyMax > 255) dutyMax = 255;
            if (dutyMax < dutyMin) dutyMax = dutyMin;
            busy = true;
            motion->clearAbort();
            gridScan(n, step, dutyMin, dutyMax);
            busy = false;
            emitStatus();
            break;
        }
        case 'A': {                                // self-calibrating two-edge dip home
            ensureMovableMode();
            char * tok  = strtok(args, " \t");
            Steps n     = tok  ? (Steps) atol(tok)  : 0;
            char * tok2 = strtok(nullptr, " \t");
            Steps step  = tok2 ? (Steps) atol(tok2) : 0;
            if (n    <= 0) n    = 2000;   // coarse +/- half-window (microsteps)
            if (step <= 0) step = 125;    // coarse position step
            busy = true;
            motion->clearAbort();
            emitLog(0, "autohome begin");
            autoHome(n, step);
            busy = false;
            emitStatus();
            break;
        }
        case 'O': {                                // fast home + backlash
            ensureMovableMode();
            char * tok  = strtok(args, " \t");
            StepsPerSecond vEdge = tok ? (StepsPerSecond) atol(tok) : 0;
            char * tok2 = strtok(nullptr, " \t");
            int m = tok2 ? atoi(tok2) : 0;
            char * tok3 = strtok(nullptr, " \t");
            StepsPerSecond vSeek = tok3 ? (StepsPerSecond) atol(tok3) : 0;
            char * tok4 = strtok(nullptr, " \t");
            StepsPerSecondPerSecond accel = tok4 ? (StepsPerSecondPerSecond) atol(tok4) : 0;
            char * tok5 = strtok(nullptr, " \t");
            bool forceCal = tok5 ? (atoi(tok5) != 0) : false;
            char * tok6 = strtok(nullptr, " \t");
            int passes = tok6 ? atoi(tok6) : 0;
            // Zeros mean "active set's default" - resolved inside fastHome so
            // a mid-run motor detection can re-resolve them for the new set.
            if (m      > 64) m      = 64;
            if (passes > 8)  passes = 8;
            busy = true;
            motion->clearAbort();
            fastHome(vEdge, m, vSeek, accel, forceCal, passes);
            busy = false;
            emitStatus();
            break;
        }
        case 'N': {                                // full-rev sensor census at fixed T
            ensureMovableMode();
            char * tok  = strtok(args, " \t");
            int T = tok ? atoi(tok) : (fastTCached > 0 ? fastTCached : (int) threshold);
            char * tok2 = strtok(nullptr, " \t");
            StepsPerSecond vmax = tok2 ? (StepsPerSecond) atol(tok2) : 20000;
            char * tok3 = strtok(nullptr, " \t");
            StepsPerSecondPerSecond accel = tok3 ? (StepsPerSecondPerSecond) atol(tok3) : mp->seekAccel;
            char * tok4 = strtok(nullptr, " \t");
            int m = tok4 ? atoi(tok4) : mp->debounceM;
            if (T < 0) T = 0; if (T > 255) T = 255;
            if (vmax <= 0) vmax = 20000;
            if (m <= 0) m = 1; if (m > 64) m = 64;
            busy = true;
            motion->clearAbort();
            censusRun(T, vmax, accel, (uint16_t) m);
            busy = false;
            emitStatus();
            break;
        }
        case 'D': {                                // driver: D <resCode 0..8> [current_mA]
            // Resolution codes = MotorDriverSettings::MicrostepResolution
            // (0=full step, 5=1/32 default). Full-step mode is the motor
            // bring-up tool: maximum torque, unambiguous 1.8-degree steps.
            char * tok = strtok(args, " \t");
            int res = tok ? atoi(tok) : -1;
            char * tok2 = strtok(nullptr, " \t");
            int mA = tok2 ? atoi(tok2) : 0;
            if (res >= 0 && res <= 8 && res != 6) {
                motorSettings->setMicrostepResolution(
                    (Modules::MotorDriverSettings::MicrostepResolution) res);
            }
            if (mA > 0) {
                motorSettings->setCurrent((float) mA / 1000.0f);   // clamped to 0.25 A in the driver module
            }
            char b[48];
            snprintf(b, sizeof(b), "D,%d,%d",
                (int) motorSettings->getMicrostepResolution(),
                (int) (motorSettings->getCurrent() * 1000.0f + 0.5f));
            testSerial.println(b);
            emitBanner();   // usteps_per_rev follows the resolution
            break;
        }
        case 'U': {                                // gear ratio: U 32|16 force, U 0 = re-detect
            char * tok = strtok(args, " \t");
            int r = tok ? atoi(tok) : -1;
            if (r == 0) {
                // Back to auto: the next O run re-measures the revolution.
                // Clear the homing caches too - "unknown motor" also means the
                // cached threshold/width anchor may belong to another module.
                motorConfirmed = false;
                fastTCached = 0;
                fastWidthAnchor = 0;
                testSerial.println("U,auto");
            } else if (r == 16 || r == 32) {
                motion->setGearRatio((uint8_t) r);
                mp = (r == 16) ? &kParams16 : &kParams32;
                motorConfirmed = true;
                fastTCached = 0;
                fastWidthAnchor = 0;
                char b[32];
                snprintf(b, sizeof(b), "U,forced,%d", r);
                testSerial.println(b);
                emitBanner();
            } else {
                testSerial.println("U,err,\"want 32, 16 or 0\"");
            }
            break;
        }
        case 'Y': {                                // ramped jog (speed/accel probing)
            ensureMovableMode();
            char * tok  = strtok(args, " \t");
            Steps delta = tok ? (Steps) atol(tok) : 0;
            char * tok2 = strtok(nullptr, " \t");
            StepsPerSecond vmax = tok2 ? (StepsPerSecond) atol(tok2) : 0;
            char * tok3 = strtok(nullptr, " \t");
            StepsPerSecondPerSecond accel = tok3
                ? (StepsPerSecondPerSecond) atol(tok3) : Bench::BENCH_RAMP_ACCEL;
            busy = true;
            motion->clearAbort();
            motion->goToRamped(motion->getPosition() + delta, vmax, accel);
            busy = false;
            emitStatus();
            break;
        }
        case 'W': {   // debug: 100-sample burst comparing the two sensor read
                      // paths (Arduino digitalRead vs the ISR's direct IDR read)
            int viaDigital = 0, viaDirect = 0;
            for (int i = 0; i < 100; i++) {
                if (motion->sensorActive())    viaDigital++;
                if (motion->sensorActiveRaw()) viaDirect++;
                delayMicroseconds(100);
            }
            char line[96];
            snprintf(line, sizeof(line), "W,%d,%d,%d,%lu,%ld",
                viaDigital, viaDirect, (int) threshold,
                (unsigned long) motion->getSensorPinMask(),
                (long) motion->getPosition());
            testSerial.println(line);
            break;
        }
        case 'P': {
            emitStatus();
            break;
        }
        case 'R': {
            char * tok = strtok(args, " \t");
            if (tok) {
                int microcode = atoi(tok);
                motorSettings->setMicrostepResolution(
                    (Modules::MotorDriverSettings::MicrostepResolution) microcode);
                char * tok2 = strtok(nullptr, " \t");
                if (tok2) {
                    motorSettings->setCurrent((float) atoi(tok2) / 1000.0f);
                }
            }
            break;
        }
        case 'J': {
            ensureMovableMode();
            char * tok = strtok(args, " \t");
            Steps delta = tok ? (Steps) atol(tok) : 0;
            char * tok2 = strtok(nullptr, " \t");
            StepsPerSecond speed = tok2 ? (StepsPerSecond) atol(tok2) : 0;
            busy = true;
            motion->clearAbort();
            motion->jog(delta, speed);
            busy = false;
            emitStatus();
            break;
        }
        case 'G': {
            ensureMovableMode();
            char * tok = strtok(args, " \t");
            Steps target = tok ? (Steps) atol(tok) : motion->getPosition();
            char * tok2 = strtok(nullptr, " \t");
            StepsPerSecond speed = tok2 ? (StepsPerSecond) atol(tok2) : 0;
            busy = true;
            motion->clearAbort();
            motion->goTo(target, speed);
            busy = false;
            emitStatus();
            break;
        }
        case 'H': {
            ensureMovableMode();
            busy = true;
            emitLog(0, "home begin");
            Bench::Motion::HomeResult res = motion->homeSideA(Bench::BENCH_SLOW_SPEED, 120000);
            busy = false;
            if (res.ok) homed = true;
            char line[128];
            snprintf(line, sizeof(line), "H,%d,%ld,%ld,%ld,%ld,\"%s\"",
                res.ok ? 1 : 0,
                (long) res.home, (long) res.switchSize,
                (long) res.leadingEdge, (long) res.trailingEdge,
                res.message);
            testSerial.println(line);
            emitStatus();
            break;
        }
        default:
            break;
    }
}

static char lineBuf[64];
static size_t lineLen = 0;
static bool lineOverflow = false;

static void pumpSerial() {
    while (testSerial.available()) {
        char c = (char) testSerial.read();
        if (c == '\n' || c == '\r') {
            if (!lineOverflow && lineLen > 0) {
                // Dispatch on a private copy and clear the shared buffer FIRST, so
                // that a re-entrant pumpSerial (via the motion service callback,
                // during a blocking move) builds its own command from a clean
                // buffer instead of appending onto this one — otherwise the first
                // mid-move command, including an abort 'X', is corrupted/dropped.
                char cmd[sizeof(lineBuf)];
                memcpy(cmd, lineBuf, lineLen);
                cmd[lineLen] = 0;
                lineLen = 0;
                lineOverflow = false;
                handleCommand(cmd);
            }
            else {
                lineLen = 0;
                lineOverflow = false;
            }
        }
        else if (lineOverflow) {
            // Discard the rest of an over-long line (don't re-parse its tail).
        }
        else if (lineLen < sizeof(lineBuf) - 1) {
            lineBuf[lineLen++] = c;
        }
        else {
            lineOverflow = true;   // over-long: drop the whole line at the newline
        }
    }
}

// Called by BenchMotion between motion poll iterations.
static void serviceTick() {
    pumpSerial();
    emitStatusThrottled();
}

// ---------------------------------------------------------------------------
// Sweep (Side A) — ascending threshold, record the crossing duty
// ---------------------------------------------------------------------------
// One ascending 0-255 threshold sweep at the CURRENT position (motor stopped).
// Returns the crossing duty (-1 = none) and the comparator state (1=HIGH) at the
// bottom/top of the range. Leaves the DAC at 255 — callers restore the threshold.
static int measureCrossingA(int& loOut, int& hiOut) {
    Modules::HomeSwitchOptical::setThreshold(0);
    delay(kSweepSettleMs);
    const bool base = sensorA->getForwardsActive();
    int cross = -1;
    bool hi = base;
    for (uint16_t d = 0; ; d += kSweepStep) {
        if (d > 255) d = 255;
        Modules::HomeSwitchOptical::setThreshold((uint8_t) d);
        delayMicroseconds(kSweepDwellUs);
        bool now = sensorA->getForwardsActive();
        hi = now;
        if (cross < 0 && now != base) cross = (int) d;
        if (d >= 255) break;
    }
    loOut = base ? 1 : 0;
    hiOut = hi ? 1 : 0;
    return cross;
}

// Accurate crossing measurement that respects the RC-filtered threshold DAC
// (tau ~100 ms). Instead of sweeping fast (which lags and can push the crossing
// of a strong on-flag signal off the top of the range), it SETTLES at each test
// point and binary-searches the flip. Returns crossing duty (-1 = none, when the
// comparator is stuck on one rail across 0..255) plus the rail states so the
// host can tell "off-flag" (stuck LOW) from "saturated/too strong" (stuck HIGH).
static int measureCrossingSettled(int& loOut, int& hiOut) {
    Modules::HomeSwitchOptical::setThreshold(0);
    delay(kSettleMs);
    const bool atLo = sensorA->getForwardsActive();
    Modules::HomeSwitchOptical::setThreshold(255);
    delay(kSettleMs);
    const bool atHi = sensorA->getForwardsActive();

    loOut = atLo ? 1 : 0;
    hiOut = atHi ? 1 : 0;
    if (atLo == atHi) {
        return -1;   // no transition in range
    }

    int lo = 0, hi = 255;
    for (int i = 0; i < 7; i++) {
        int mid = (lo + hi) / 2;
        Modules::HomeSwitchOptical::setThreshold((uint8_t) mid);
        delay(kSettleMs);
        if (sensorA->getForwardsActive() == atLo) {
            lo = mid;   // transition is still above mid
        } else {
            hi = mid;
        }
    }
    return (lo + hi) / 2;
}

static void doSweepA() {
    int lo, hi;
    int cross = measureCrossingA(lo, hi);
    if (cross >= 0) sweepPush((uint16_t) cross);

    char line[80];
    snprintf(line, sizeof(line), "D,%lu,%d,-1,%d,0,%d,%d,0,0",
        (unsigned long) millis(), cross,
        (int) lroundf(sweepStdev() * 10.0f), lo, hi);
    testSerial.println(line);
}

// Coarse-to-fine peak-crossing search over +/-N microsteps around the current
// position. Emits a Q,<pos>,<cross> line per sample for live plotting, then a
// B,<bestPos>,<bestCross>,<threshold>. Moves to the peak and stores the
// threshold = peak crossing (the chosen calibration rule). Abortable via X.
static void searchPeak(Steps N) {
    if (N <= 0) N = 500;
    const StepsPerSecond speed = motion->getDefaultSpeed();

    // Coarse-to-fine, fixed sample counts (the settled measure is ~2 s/sample, so
    // keep the total sample count modest). Level 1 refines around the best.
    struct Level { Steps range; int points; };
    Level levels[2] = {
        { N,                          7 },
        { (N / 3) > 0 ? (N / 3) : 20, 7 },
    };

    Steps bestPos = motion->getPosition();
    int   bestCross = -1;
    Steps center = motion->getPosition();

    for (int li = 0; li < 2; li++) {
        Steps range = levels[li].range;
        int   npts  = levels[li].points;
        for (int i = 0; i < npts; i++) {
            Steps p = center - range + (Steps)((2L * (long) range * i) / (npts - 1));
            motion->goTo(p, speed);
            if (motion->aborted()) { emitLog(1, "search aborted"); return; }
            int lo, hi;
            int cross = measureCrossingSettled(lo, hi);
            char line[48];
            snprintf(line, sizeof(line), "Q,%ld,%d", (long) p, cross);
            testSerial.println(line);
            if (cross > bestCross) { bestCross = cross; bestPos = p; }
        }
        center = bestPos;   // refine around the best so far
    }

    motion->goTo(bestPos, speed);
    if (bestCross >= 0) {
        threshold = (uint8_t) bestCross;
    }
    Modules::HomeSwitchOptical::setThreshold(threshold);

    char line[80];
    snprintf(line, sizeof(line), "B,%ld,%d,%d",
        (long) bestPos, bestCross, (int) threshold);
    testSerial.println(line);
}

// ---------------------------------------------------------------------------
// Home-shape grid scan (command K)
// ---------------------------------------------------------------------------
// Pump the serial parser + throttled status while waiting `ms`, honouring abort.
// Returns false as soon as an abort is requested, so long dwells stay responsive
// to 'X' (~one dwell) and the 'S' status stream keeps flowing during the scan.
static bool waitServiced(uint32_t ms) {
    uint32_t start = millis();
    while ((uint32_t) (millis() - start) < ms) {
        serviceTick();                     // pumpSerial() + emitStatusThrottled()
        if (motion->aborted()) return false;
        delay(1);
    }
    return true;
}

// One strictly-ascending threshold sweep over [dutyMin, dutyMax] at the CURRENT
// position. The duty only ever rises, so the comparator's HIGH->LOW flip is always
// caught on the same hysteresis branch => a faithful position->strength shape across
// the whole scan. Each duty step dwells `dwellMs` (RC-serviced), so the timing lag
// is CONSISTENT at every position (it warps only the Y axis, never the shape).
// Returns the crossing duty (-1 = none). loOut/hiOut are the comparator states at
// the bottom (dutyMin, after settle) / top of the range:
//   lo==hi==0 -> stuck LOW  (off flag / no reflection)
//   lo==hi==1 -> stuck HIGH (saturated / reflection too strong to bracket)
// NB dutyMin must sit BELOW the lowest real crossing (~200 on this bench), else a
// dip point already flipped at dutyMin would misclassify as stuck-HIGH.
static int measureCrossingAscending(int dutyMin, int dutyMax, int dutyStep,
                                    uint32_t settleMs, uint32_t dwellMs,
                                    int& loOut, int& hiOut) {
    if (dutyStep < 1) dutyStep = 1;
    if (dutyMin < 0) dutyMin = 0;
    if (dutyMax > 255) dutyMax = 255;
    if (dutyMax < dutyMin) dutyMax = dutyMin;

    // Force the low branch AT dutyMin and let the RC drain (~2 tau) so `base` is the
    // firmly-settled state at dutyMin - the consistent starting point each time.
    Modules::HomeSwitchOptical::setThreshold((uint8_t) dutyMin);
    waitServiced(settleMs);
    const bool base = sensorA->getForwardsActive();
    loOut = base ? 1 : 0;

    int  cross = -1;
    bool top   = base;
    for (int d = dutyMin; ; d += dutyStep) {
        if (d > dutyMax) d = dutyMax;
        Modules::HomeSwitchOptical::setThreshold((uint8_t) d);
        if (!waitServiced(dwellMs)) break;         // aborted mid-sweep
        bool now = sensorA->getForwardsActive();
        top = now;
        if (now != base) { cross = d; break; }     // first ascending flip
        if (d >= dutyMax) break;                    // reached the top rail, no flip
    }
    hiOut = top ? 1 : 0;
    return cross;
}

// Uniform position grid over +/-n microsteps in `step` increments around the
// current position. At each position, one ascending threshold sweep -> crossing
// duty (a reflection-strength proxy). Streams K,begin / K,<i> / K,end for live
// plotting. Unlike searchPeak it keeps the WHOLE curve and does NOT change the
// operating threshold. The low end is approached from below (forward take-up) so
// the whole sweep runs on one gear flank (cf. homeSideA's single forward pass).
static void gridScan(Steps n, Steps step, int dutyMin, int dutyMax) {
    if (n    <= 0) n    = 500;
    if (step <= 0) step = 10;
    const StepsPerSecond speed = motion->getDefaultSpeed();
    const Steps center = motion->getPosition();
    const int   count  = (int) ((2L * (long) n) / (long) step) + 1;

    char line[96];
    snprintf(line, sizeof(line), "K,begin,%ld,%ld,%ld,%d,%d,%d",
        (long) center, (long) n, (long) step, count, dutyMin, dutyMax);
    testSerial.println(line);

    // Approach center-n from BELOW, taking up gear backlash, so every subsequent
    // sample move is forward and shares one engagement frame.
    motion->goTo(center - n - kScanBacklash, speed);
    motion->goTo(center - n, speed);

    int idx = 0;
    if (!motion->aborted()) {
        for (Steps p = center - n; p <= center + n; p += step) {
            motion->goTo(p, speed);
            if (motion->aborted()) break;
            int lo, hi;
            int cross = measureCrossingAscending(dutyMin, dutyMax, kScanDutyStep,
                                                 kScanSettleMs, kScanDwellMs, lo, hi);
            if (motion->aborted()) break;          // discard the interrupted sample
            snprintf(line, sizeof(line), "K,%d,%ld,%d,%d,%d",
                idx, (long) motion->getPosition(), cross, lo, hi);
            testSerial.println(line);
            idx++;
        }
    }

    const int abortedFlag = motion->aborted() ? 1 : 0;
    Modules::HomeSwitchOptical::setThreshold(threshold);   // restore operating DAC
    motion->clearAbort();                                  // allow the park move
    motion->goTo(center, speed);                           // return to centre

    snprintf(line, sizeof(line), "K,end,%ld,%d,%d",
        (long) center, idx, abortedFlag);
    testSerial.println(line);
}

// ---------------------------------------------------------------------------
// Self-calibrating two-edge dip home (command A)
// ---------------------------------------------------------------------------
// Restore the operating DAC, clear abort, park, and report failure (A,done,0,...).
static void autoHomeFail(const char * msg, Steps parkTo) {
    Modules::HomeSwitchOptical::setThreshold(threshold);
    motion->clearAbort();
    motion->goTo(parkTo, motion->getDefaultSpeed());
    char line[96];
    snprintf(line, sizeof(line), "A,done,0,0,0,0,0,%d,0,\"%s\"", (int) threshold, msg);
    testSerial.println(line);
}

// Self-calibrating homing. The home feature is a DIP in crossing-duty (weak
// reflection) between reflective shoulders. Phase 1 coarsely characterises the dip
// (restricted duty, fast) to find its minimum crossing `cmin`; phase 2 picks a
// threshold T into the upper-middle of the dip so the dip reads ACTIVE (crossing<T)
// and the shoulders INACTIVE - a normal two-edge flag; phase 3 sets T and homes to
// the midpoint of the two edges (a threshold hit is an EDGE, not the centre - so we
// take BOTH edges, like the mechanical switch). Adopts T as the operating threshold
// (220 is below the dip and cannot see it). Uses no values from the manual scan.
static void autoHome(Steps n, Steps step) {
    const StepsPerSecond speed = motion->getDefaultSpeed();
    const Steps center = motion->getPosition();

    // Clamp coarse-point count to the static buffer.
    Steps effStep = step;
    if ((2L * (long) n) / (long) effStep + 1 > kAutoMaxPoints)
        effStep = (Steps) ((2L * (long) n) / (long) (kAutoMaxPoints - 1)) + 1;

    char line[96];
    snprintf(line, sizeof(line), "A,begin,%ld,%ld,%ld,%d,%d",
        (long) center, (long) n, (long) effStep, kScanDutyMin, 255);
    testSerial.println(line);

    // ---- Phase 1: coarse characterisation, forward take-up (single flank) ----
    motion->goTo(center - n - kScanBacklash, speed);
    motion->goTo(center - n, speed);

    int np = 0;
    if (!motion->aborted()) {
        for (Steps p = center - n; p <= center + n && np < kAutoMaxPoints; p += effStep) {
            motion->goTo(p, speed);
            if (motion->aborted()) break;
            int lo, hi;
            int cross = measureCrossingAscending(kScanDutyMin, 255, kScanDutyStep,
                                                 kScanSettleMs, kScanDwellMs, lo, hi);
            if (motion->aborted()) break;
            autoPos[np] = motion->getPosition();
            autoCross[np] = cross;
            snprintf(line, sizeof(line), "A,pt,%d,%ld,%d,%d,%d",
                np, (long) autoPos[np], cross, lo, hi);
            testSerial.println(line);
            np++;
        }
    }
    if (motion->aborted()) { autoHomeFail("abort", center); return; }

    // Find the dip minimum from a 3-point median-smoothed series (noise-robust).
    // median(a,b,c) = a + b + c - max - min.
    int   cmin = 256;
    Steps pmin = center;
    for (int i = 0; i < np; i++) {
        if (autoCross[i] < 0) continue;                 // skip stuck / off-flag
        int a = autoCross[i];
        int b = (i > 0      && autoCross[i - 1] >= 0) ? autoCross[i - 1] : a;
        int c = (i < np - 1 && autoCross[i + 1] >= 0) ? autoCross[i + 1] : a;
        int hiv = a > b ? a : b; if (c > hiv) hiv = c;   // max(a,b,c)
        int lov = a < b ? a : b; if (c < lov) lov = c;   // min(a,b,c)
        int med = a + b + c - hiv - lov;                 // median of 3
        if (med < cmin) { cmin = med; pmin = autoPos[i]; }
    }
    if (cmin > 255) { autoHomeFail("no dip found", center); return; }
    const int dipDepth = 255 - cmin;
    if (dipDepth < kAutoMinDipDepth) { autoHomeFail("dip too shallow", center); return; }

    // ---- Phase 2: threshold into the upper-middle of the dip ----
    int T = cmin + (int) lroundf((255.0f - (float) cmin) * kAutoThreshFrac);
    if (T < cmin + 1) T = cmin + 1;
    if (T > 254)      T = 254;
    snprintf(line, sizeof(line), "A,thr,%d,%d,%ld,%d", T, cmin, (long) pmin, dipDepth);
    testSerial.println(line);

    // Approx the active band's leading edge from the coarse points -> start point.
    Steps leadApprox = center - n;
    bool  haveBand = false;
    for (int i = 0; i < np; i++) {
        if (autoCross[i] >= 0 && autoCross[i] < T) { leadApprox = autoPos[i]; haveBand = true; break; }
    }
    if (!haveBand) leadApprox = pmin;

    // ---- Phase 3: set T, settle the RC, one forward two-edge pass ----
    Modules::HomeSwitchOptical::setThreshold((uint8_t) T);
    waitServiced(kAutoThresholdSettleMs);

    const Steps clearance = (Steps)(20000 / 128) * motion->getMicrostepsPerStep();  // == homeSideA's
    const uint32_t deadline = millis() + kAutoHomeTimeoutMs;

    // Start OFF the flag, below the leading edge, backlash taken up forwards.
    Steps startBelow = leadApprox - effStep - clearance;
    motion->goTo(startBelow - kScanBacklash, speed);
    motion->goTo(startBelow, speed);
    if (motion->aborted()) { autoHomeFail("abort", center); return; }
    if (motion->sensorActive())                        // guard: must begin inactive
        motion->goTo(motion->getPosition() - clearance, speed);

    bool ok1;
    Steps leadingEdge = motion->approachEdge(true, true, Bench::BENCH_SLOW_SPEED, deadline, ok1);
    if (!ok1) { autoHomeFail(motion->aborted() ? "abort" : "timeout leading", center); return; }
    snprintf(line, sizeof(line), "A,edge,0,%ld", (long) leadingEdge);
    testSerial.println(line);

    bool ok2;
    Steps trailingEdge = motion->approachEdge(true, false, Bench::BENCH_SLOW_SPEED, deadline, ok2);
    if (!ok2) { autoHomeFail(motion->aborted() ? "abort" : "timeout trailing", center); return; }
    snprintf(line, sizeof(line), "A,edge,1,%ld", (long) trailingEdge);
    testSerial.println(line);

    const Steps home       = (leadingEdge + trailingEdge) / 2;
    const Steps switchSize = trailingEdge - leadingEdge;

    // Adopt T as the live operating threshold (220 makes the dip invisible), relabel
    // home as zero, and park there for a visual check.
    threshold = (uint8_t) T;
    Modules::HomeSwitchOptical::setThreshold(threshold);
    motion->goTo(home, speed);
    motion->zero();
    homed = true;

    snprintf(line, sizeof(line), "A,done,1,%ld,%ld,%ld,%ld,%d,%d,\"ok\"",
        (long) home, (long) leadingEdge, (long) trailingEdge, (long) switchSize, T, dipDepth);
    testSerial.println(line);
}

// ---------------------------------------------------------------------------
// Fast home + backlash (command O)
// ---------------------------------------------------------------------------
// Restore the operating DAC, clear abort, and report failure. Does NOT park -
// on failure the stage stays where it stopped (faster iteration, and the host
// can inspect the failure position). Clears the threshold cache: a failure
// usually means the reflection profile drifted, so the next run recalibrates.
static void fastHomeFail(const char * msg, uint32_t t0) {
    fastTCached = 0;
    Modules::HomeSwitchOptical::setThreshold(threshold);
    motion->clearAbort();
    char line[112];
    snprintf(line, sizeof(line), "O,done,0,0,0,0,0,0,%d,%lu,\"%s\"",
        (int) threshold, (unsigned long) (millis() - t0), msg);
    testSerial.println(line);
}

// Lean settled crossing probe: binary-search the comparator flip over
// [lo0, 255] with a true RC settle per probe (accurate, unlike the fast
// ascending sweeps whose readings lag ~20+ duty). ~1.5 s at 5 iterations.
// Returns the crossing, or -1 when stuck (rails in loOut/hiOut), or -2 on
// abort. The DAC is left at the last probe - callers must restore it.
static int measureCrossingSettledFast(int lo0, int& loOut, int& hiOut) {
    loOut = hiOut = -1;
    Modules::HomeSwitchOptical::setThreshold((uint8_t) lo0);
    if (!waitServiced(kFastCalSettleMs)) return -2;
    const bool atLo = sensorA->getForwardsActive();
    Modules::HomeSwitchOptical::setThreshold(255);
    if (!waitServiced(kFastCalSettleMs)) return -2;
    const bool atHi = sensorA->getForwardsActive();
    loOut = atLo ? 1 : 0;
    hiOut = atHi ? 1 : 0;
    if (atLo == atHi) return -1;
    int lo = lo0, hi = 255;
    for (int i = 0; i < kFastCalIters; i++) {
        int mid = (lo + hi) / 2;
        Modules::HomeSwitchOptical::setThreshold((uint8_t) mid);
        if (!waitServiced(kFastCalSettleMs)) return -2;
        if (sensorA->getForwardsActive() == atLo) lo = mid; else hi = mid;
    }
    return (lo + hi) / 2;
}

// Map a stuck probe onto the bracket: never-active = crossing above 255
// (treat as 255), always-active = below the bracket floor (treat as lo0).
static int normCrossing(int c, int loRail) {
    if (c >= 0) return c;
    return loRail ? kFastCalBracketLo : 255;
}

// One forward two-edge pass at vEdge, with the flank-blip retry loop (a
// dither blip latching as a tiny "flag" is re-armed past in place). Emits
// O,edge lines. The caller must have positioned the stage forward-engaged,
// off-flag, below the leading edge. Returns false on abort/timeout.
static bool fastTwoEdgePass(StepsPerSecond vEdge, uint32_t deadline,
                            Steps& leadOut, Steps& trailOut,
                            const char *& failMsg) {
    char line[64];
    bool ok1, ok2;
    for (int blipSkips = 0; ; blipSkips++) {
        leadOut = motion->approachEdge(true, true, vEdge, deadline, ok1);
        if (!ok1) {
            failMsg = motion->aborted() ? "abort" : "timeout leading";
            return false;
        }
        snprintf(line, sizeof(line), "O,edge,0,%ld", (long) leadOut);
        testSerial.println(line);

        trailOut = motion->approachEdge(true, false, vEdge, deadline, ok2);
        if (!ok2) {
            failMsg = motion->aborted() ? "abort" : "timeout trailing";
            return false;
        }
        snprintf(line, sizeof(line), "O,edge,1,%ld", (long) trailOut);
        testSerial.println(line);

        if (trailOut - leadOut >= mp->widthMin || blipSkips >= 3) {
            return true;
        }
        snprintf(line, sizeof(line), "O,gate,width,0,%ld",
            (long) (trailOut - leadOut));
        testSerial.println(line);
    }
}

// The production-equivalent routine (homeRoutine + measureBacklashRoutine in
// one pass), fast:
//   0. CALIBRATE - (cold runs only) measure the local background crossing at
//                two spots and set T_op = background - margin: background
//                reads INACTIVE, only the deep dip reads ACTIVE. Cached.
//   1. SEEK    - ramped run (up to vSeek) with the debounced ISR latch armed;
//                finds a coarse leading edge anywhere in ~1.25 revolutions.
//                Self-heals a drifted threshold: cannot-clear -> T down,
//                nothing-found -> T up (bounded).
//   1b. DETECT - (gear ratio unconfirmed, or forceCal) continue the seek one
//                more lap to the NEXT leading edge: the lead-to-lead distance
//                is one full revolution, classifying the module as 32:1
//                (~189,704 usteps) or 16:1 (~94,852). Selects the matching
//                MotorParams set + steps/rev and re-prints the banner.
//   2. GATES   - depth: settled crossing at the estimated centre must sit
//                kFastDepthBelowBg below the anchor background (cold runs;
//                decided at the arming point where the anchor is measured);
//                shoulder: a clearance below the lead must read INACTIVE;
//                width: the measured flag width must be plausible.
//   3. PRECISE - one forward pass at vEdge latching leading + trailing edge in
//                the same engaged frame; home = midpoint (backlash-free).
//   4. BACKLASH- walk past the trailing edge (no reversal), verify disengaged,
//                reverse and latch re-entry: backlash = trail - reenter (same
//                arithmetic as the production measureBacklashRoutine).
//   5. PARK    - forward-engaged approach to home, exact frame shift (no
//                park-overrun error), adopt T_op as the operating threshold.
static void fastHome(StepsPerSecond vEdge, int m, StepsPerSecond vSeek,
                     StepsPerSecondPerSecond accel, bool forceCal, int passes) {
    const uint32_t t0 = millis();
    const uint32_t deadline = t0 + kFastTimeoutMs;
    char line[128];

    // Zero args mean "the active set's default". Remember which were
    // defaulted: if the detection phase switches sets mid-run, those are
    // re-resolved against the new set (explicit values always win).
    const bool defVEdge = (vEdge <= 0), defM = (m <= 0),
               defVSeek = (vSeek <= 0), defAccel = (accel <= 0);
    if (defVEdge) vEdge = mp->edgeSpeed;
    if (defM)     m     = mp->debounceM;
    if (defVSeek) vSeek = mp->seekSpeed;
    if (defAccel) accel = mp->seekAccel;
    if (defVSeek && !motorConfirmed) {
        // The attached motor could be either generation: seek at the slower
        // of the two cruise speeds until it is identified (the 16:1 motor
        // stalls hard at the 32:1 cruise - 17-19k cliff vs 24k).
        if (kParams16.seekSpeed < vSeek) vSeek = kParams16.seekSpeed;
        if (kParams32.seekSpeed < vSeek) vSeek = kParams32.seekSpeed;
    }
    if (passes <= 0) passes = kFastPassesDefault;

    Steps rev = motion->getMicrostepsPerPrismRotation();
    Steps travelBudgetTotal = rev + rev / 4;   // 1.25 rev per seek attempt

    motion->setLatchDebounce((uint16_t) m);
    motion->enable(true);

    const bool warm = (!forceCal && fastTCached > 0);
    int T = warm ? fastTCached : 0;

    snprintf(line, sizeof(line), "O,begin,%ld,%d,%ld,%d,%ld,%d",
        (long) motion->getPosition(), T, (long) vEdge, m, (long) vSeek, passes);
    testSerial.println(line);

    // ---- Phase 0: threshold calibration (cold runs) --------------------------
    if (!warm) {
        int lo1, hi1, lo2, hi2;
        int c1 = measureCrossingSettledFast(kFastCalBracketLo, lo1, hi1);
        if (c1 == -2) { fastHomeFail("abort", t0); return; }
        motion->goToRamped(motion->getPosition() + 3000, mp->reposSpeed, accel);
        if (motion->aborted()) { fastHomeFail("abort", t0); return; }
        int c2 = measureCrossingSettledFast(kFastCalBracketLo, lo2, hi2);
        if (c2 == -2) { fastHomeFail("abort", t0); return; }
        const int b1 = normCrossing(c1, lo1);
        const int b2 = normCrossing(c2, lo2);
        const int bg = b1 > b2 ? b1 : b2;
        T = bg - kFastBgMargin;
        if (T < 16)  T = 16;
        if (T > 250) T = 250;
        snprintf(line, sizeof(line), "O,cal,%d,%d,%d,%d", b1, b2, bg, T);
        testSerial.println(line);
    }
    Modules::HomeSwitchOptical::setThreshold((uint8_t) T);
    if (!waitServiced(kFastSettleMs)) { fastHomeFail("abort", t0); return; }

    int rejects = 0;
    int tAdjusts = 0;
    const Steps seekStart = motion->getPosition();

    // ---- Phase 1: land T in-band and find a coarse leading edge -------------
    Steps coarseLead;
    while (true) {
        if (motion->sensorActive()) {
            // Active at rest: either on the dip, or T is above the local
            // background. Exit backward, BOUNDED - an unbounded clear at a
            // too-high threshold never terminates (whole rev active).
            const Steps bound = motion->getPosition()
                - (mp->widthMax + mp->clearance);
            if (motion->moveUntilSensor(false, false, mp->reposSpeed,
                    true, bound, deadline)) {
                coarseLead = motion->getPosition();
                break;
            }
            if (motion->aborted() || millis() > deadline) {
                fastHomeFail(motion->aborted() ? "abort" : "timeout clearing", t0);
                return;
            }
            // Still active after the bound: background is active -> T too high.
            if (++tAdjusts > kFastMaxTAdjust) {
                fastHomeFail("no inactive background", t0);
                return;
            }
            T -= kFastTAdjustDown;
            if (T < 16) { fastHomeFail("threshold floor", t0); return; }
            Modules::HomeSwitchOptical::setThreshold((uint8_t) T);
            if (!waitServiced(kFastGateSettleMs)) { fastHomeFail("abort", t0); return; }
            snprintf(line, sizeof(line), "O,thr,%d,down", T);
            testSerial.println(line);
            continue;
        }

        // Inactive at rest: seek forward for the dip.
        bool okSeek;
        coarseLead = motion->seekLatch(true, true, vSeek, accel,
            travelBudgetTotal, deadline, okSeek);
        if (okSeek) break;
        if (motion->aborted() || millis() > deadline) {
            fastHomeFail(motion->aborted() ? "abort" : "timeout seeking", t0);
            return;
        }
        // A whole lap with nothing active: T is below the dip floor.
        if (++tAdjusts > kFastMaxTAdjust) {
            fastHomeFail("flag not found", t0);
            return;
        }
        T += kFastTAdjustUp;
        if (T > 250) { fastHomeFail("threshold ceiling", t0); return; }
        Modules::HomeSwitchOptical::setThreshold((uint8_t) T);
        if (!waitServiced(kFastGateSettleMs)) { fastHomeFail("abort", t0); return; }
        snprintf(line, sizeof(line), "O,thr,%d,up", T);
        testSerial.println(line);
    }
    snprintf(line, sizeof(line), "O,seek,%ld", (long) coarseLead);
    testSerial.println(line);

    // ---- Phase 1b: motor detection (lead-to-lead = one revolution) ----------
    // Classification only needs coarse edges: the candidate revs differ by a
    // factor of 2 (~95k usteps) while seek-lag / backlash / branch asymmetries
    // contribute well under 2k. The precise steps/rev then comes from the
    // selected set's exact rational, not from this measurement.
    if (!motorConfirmed || forceCal) {
        bool okD;
        if (motion->sensorActive()) {
            motion->seekLatch(true, false, vSeek, accel,
                mp->widthMax + mp->takeup, deadline, okD);
            if (!okD) {
                fastHomeFail(motion->aborted() ? "abort" : "stuck active", t0);
                return;
            }
        }
        motion->goToRamped(motion->getPosition() + mp->takeup, mp->reposSpeed, accel);
        if (motion->aborted()) { fastHomeFail("abort", t0); return; }
        const Steps lead2 = motion->seekLatch(true, true, vSeek, accel,
            travelBudgetTotal, deadline, okD);
        if (!okD) {
            fastHomeFail(motion->aborted() ? "abort" : "motor detect: flag not found", t0);
            return;
        }
        const Steps measuredRev = lead2 - coarseLead;
        const Steps usteps = motion->getMicrostepsPerStep();
        uint8_t ratio = 0;
        for (uint8_t cand = 16; cand <= 32; cand += 16) {
            const Steps expect = Bench::Motion::microstepsPerPrismRotationFor(cand, usteps);
            if (measuredRev > expect - expect / 10 && measuredRev < expect + expect / 10) {
                ratio = cand;
            }
        }
        snprintf(line, sizeof(line), "O,motor,%d,%ld", (int) ratio, (long) measuredRev);
        testSerial.println(line);
        if (ratio == 0) { fastHomeFail("motor detect out of range", t0); return; }

        motion->setGearRatio(ratio);
        mp = (ratio == 16) ? &kParams16 : &kParams32;
        motorConfirmed = true;
        emitBanner();

        rev = motion->getMicrostepsPerPrismRotation();
        travelBudgetTotal = rev + rev / 4;
        if (defVEdge) vEdge = mp->edgeSpeed;
        if (defM)     { m = mp->debounceM; motion->setLatchDebounce((uint16_t) m); }
        if (defVSeek) vSeek = mp->seekSpeed;
        if (defAccel) accel = mp->seekAccel;

        coarseLead = lead2;   // continue the routine from the re-found lead
    }

    // ---- Phase 2+3: gates and the precise pass (with false-feature retry) ---
    Steps lead = 0, trail = 0;
    int coldDepth = -1;
    while (true) {
        bool accepted = true;

        // Depth probe (cold runs; warm runs trust the cache - the shoulder and
        // width gates below still protect): settled crossing at the estimated
        // centre. Measure only - the verdict needs the anchor background,
        // measured at the arming point below (the probe sits ~200 usteps past
        // the true centre because the seek lag shifts coarseLead forward, so
        // it reads a shallow flank; judging it against T, which itself sits
        // only ~4 counts above the floor, false-rejected real flags).
        if (!warm) {
            motion->goToRamped(coarseLead + mp->halfWidthGuess, mp->reposSpeed, accel);
            if (motion->aborted()) { fastHomeFail("abort", t0); return; }
            int glo, ghi;
            int cc = measureCrossingSettledFast(kFastCalBracketLo, glo, ghi);
            if (cc == -2) { fastHomeFail("abort", t0); return; }
            coldDepth = normCrossing(cc, glo);
            Modules::HomeSwitchOptical::setThreshold((uint8_t) T);
            if (!waitServiced(kFastGateSettleMs)) { fastHomeFail("abort", t0); return; }
        }

        if (accepted) {
            // Reposition below the lead, forward-engaged; the moves double as
            // most of the RC settle back to T, topped up before arming.
            motion->goToRamped(coarseLead - mp->clearance - mp->takeup, mp->reposSpeed, accel);
            motion->goToRamped(coarseLead - mp->clearance, mp->reposSpeed, accel);
            if (motion->aborted()) { fastHomeFail("abort", t0); return; }
            if (!waitServiced(kFastGateSettleMs / 2)) { fastHomeFail("abort", t0); return; }

            if (!warm) {
                // Threshold anchor (cold runs): the background level varies by
                // ring sector, so a T calibrated at the arbitrary start
                // position is bistable (bench: 231 vs 239; at 239 the flag
                // reads near its shoulders - ~2x width, ~2.5x backlash, 10x
                // worse repeatability). Re-derive T at THIS spot, the same
                // fixed offset below the lead every run.
                int alo, ahi;
                int ac = measureCrossingSettledFast(kFastCalBracketLo, alo, ahi);
                if (ac == -2) { fastHomeFail("abort", t0); return; }
                const int anchorBg = normCrossing(ac, alo);
                int Tc = anchorBg - kFastBgMargin;
                if (Tc < 16)  Tc = 16;
                if (Tc > 250) Tc = 250;
                if (Tc != T) {
                    T = Tc;
                    snprintf(line, sizeof(line), "O,thr,%d,anchor", T);
                    testSerial.println(line);
                }
                // Depth verdict: the dip must sit well below the LOCAL
                // background (floor is 10-14 counts below it; false features
                // read within 2-3). Robust against hour-scale profile drift.
                if (coldDepth >= 0) {
                    accepted = (coldDepth <= anchorBg - kFastDepthBelowBg);
                    snprintf(line, sizeof(line), "O,gate,depth,%d,%d",
                        accepted ? 1 : 0, coldDepth);
                    testSerial.println(line);
                }
                Modules::HomeSwitchOptical::setThreshold((uint8_t) T);
                if (!waitServiced(kFastGateSettleMs)) { fastHomeFail("abort", t0); return; }
            }

            if (!accepted) {
                // fall through to the reject path below
            } else if (motion->sensorActive()) {
                // Shoulder gate: a clearance below the lead must read off-flag.
                accepted = false;
                snprintf(line, sizeof(line), "O,gate,shoulder,0,0");
                testSerial.println(line);
            } else {
                Steps passLead, passTrail;
                const char * why;
                if (!fastTwoEdgePass(vEdge, deadline, passLead, passTrail, why)) {
                    fastHomeFail(why, t0);
                    return;
                }

                const Steps width = passTrail - passLead;
                const bool widthOk = (width >= mp->widthMin && width <= mp->widthMax);
                if (!widthOk) {
                    accepted = false;
                    coarseLead = passLead;   // reject relative to the measured feature
                    snprintf(line, sizeof(line), "O,gate,width,0,%ld", (long) width);
                    testSerial.println(line);
                } else {
                    lead = passLead;
                    trail = passTrail;
                    break;   // accepted - fall through to backlash
                }
            }
        }

        // Rejected: skip past this feature and re-arm the seek. Each attempt
        // gets a fresh lap budget; the rejects counter bounds the total work.
        if (++rejects > kFastMaxRejects) { fastHomeFail("too many false flags", t0); return; }
        {
            bool okX;
            if (motion->sensorActive()) {
                motion->seekLatch(true, false, vSeek, accel,
                    mp->widthMax + mp->takeup, deadline, okX);
                if (!okX) {
                    fastHomeFail(motion->aborted() ? "abort" : "stuck active", t0);
                    return;
                }
            }
            motion->goToRamped(motion->getPosition() + mp->takeup, mp->reposSpeed, accel);
            coarseLead = motion->seekLatch(true, true, vSeek, accel,
                travelBudgetTotal, deadline, okX);
            if (!okX) {
                fastHomeFail(motion->aborted() ? "abort" : "flag not found", t0);
                return;
            }
            snprintf(line, sizeof(line), "O,seek,%ld", (long) coarseLead);
            testSerial.println(line);
        }
    }

    // ---- Additional precise passes (average out per-pass noise) -------------
    // Passes 2..N re-run the same forward-engaged two-edge pass; the combined
    // estimate is the mean of the midpoints (median from 3 passes up - and two
    // passes that disagree badly trigger a tie-breaking third).
    Steps mids[8], widths[8];
    int npass = 0;
    mids[npass] = (lead + trail) / 2;
    widths[npass] = trail - lead;
    snprintf(line, sizeof(line), "O,pass,0,%ld,%ld", (long) lead, (long) trail);
    testSerial.println(line);
    npass++;

    const Steps passAnchor = lead;   // stable repositioning target
    int wantPasses = passes;
    while (npass < wantPasses && npass < 8) {
        motion->goToRamped(passAnchor - mp->clearance - mp->takeup, mp->reposSpeed, accel);
        motion->goToRamped(passAnchor - mp->clearance, mp->reposSpeed, accel);
        if (motion->aborted()) { fastHomeFail("abort", t0); return; }
        if (motion->sensorActive()) {
            fastHomeFail("active at pass arming point", t0);
            return;
        }
        Steps l, tr;
        const char * why;
        if (!fastTwoEdgePass(vEdge, deadline, l, tr, why)) {
            fastHomeFail(why, t0);
            return;
        }
        if (tr - l < mp->widthMin || tr - l > mp->widthMax) {
            fastHomeFail("width gate on repeat pass", t0);
            return;
        }
        mids[npass] = (l + tr) / 2;
        widths[npass] = tr - l;
        snprintf(line, sizeof(line), "O,pass,%d,%ld,%ld", npass, (long) l, (long) tr);
        testSerial.println(line);
        npass++;
        lead = l;      // O,done reports the last pass's edges
        trail = tr;

        if (npass == 2 && wantPasses == 2) {
            Steps d = mids[1] - mids[0];
            if (d < 0) d = -d;
            if (d > kFastPassDisagree) wantPasses = 3;   // tie-breaker
        }
    }

    Steps home;
    if (npass == 1) {
        home = mids[0];
    } else if (npass == 2) {
        home = (mids[0] + mids[1]) / 2;
    } else {
        for (int i = 1; i < npass; i++) {          // insertion sort (n <= 8)
            Steps v = mids[i];
            int j = i - 1;
            while (j >= 0 && mids[j] > v) { mids[j + 1] = mids[j]; j--; }
            mids[j + 1] = v;
        }
        home = (npass % 2) ? mids[npass / 2]
                           : (mids[npass / 2 - 1] + mids[npass / 2]) / 2;
    }
    long widthSum = 0;
    for (int i = 0; i < npass; i++) widthSum += widths[i];
    const Steps switchSize = (Steps) (widthSum / npass);

    // ---- Phase 4: backlash at the trailing edge -----------------------------
    Steps walkTo = trail + mp->backlashWalk;
    for (int extend = 0; ; extend++) {
        motion->goToRamped(walkTo, mp->reposSpeed, accel);
        if (motion->aborted()) { fastHomeFail("abort", t0); return; }
        if (!motion->sensorActive()) break;   // properly disengaged
        if (extend >= 2) { fastHomeFail("no disengage after trailing edge", t0); return; }
        walkTo += mp->backlashWalk / 2;
    }
    bool okB;
    const Steps reenter = motion->approachEdge(false, true, vEdge, deadline, okB);
    if (!okB) {
        fastHomeFail(motion->aborted() ? "abort" : "timeout backlash", t0);
        return;
    }
    Steps backlash = trail - reenter;
    snprintf(line, sizeof(line), "O,backlash,%ld,%ld", (long) backlash, (long) reenter);
    testSerial.println(line);
    if (backlash < -32 || backlash > mp->backlashMax) {
        fastHomeFail("backlash out of range", t0);
        return;
    }
    if (backlash < 0) backlash = 0;   // tiny negative = quantisation slack only

    // ---- Phase 5: park at home, forward-engaged, exact frame shift ----------
    motion->goToRamped(home - mp->clearance - mp->takeup, mp->reposSpeed, accel);
    motion->goToRamped(home - mp->clearance, mp->reposSpeed, accel);
    motion->goToRamped(home, mp->reposSpeed, accel);
    if (motion->aborted()) { fastHomeFail("abort", t0); return; }

    motion->shiftFrame(home);
    threshold = (uint8_t) T;
    Modules::HomeSwitchOptical::setThreshold(threshold);
    fastTCached = T;
    homed = true;

    // Width servo (see fastWidthAnchor): trim the NEXT run's threshold by one
    // count when the width has drifted out of the band around the anchor.
    if (!warm || fastWidthAnchor == 0) {
        fastWidthAnchor = switchSize;              // (re)anchor on cold runs
    } else if (switchSize < fastWidthAnchor - kFastWidthServoBand && fastTCached < 250) {
        fastTCached++;                             // profile rose: widen back
        snprintf(line, sizeof(line), "O,thr,%d,servo", fastTCached);
        testSerial.println(line);
    } else if (switchSize > fastWidthAnchor + kFastWidthServoBand && fastTCached > 16) {
        fastTCached--;                             // profile fell: narrow back
        snprintf(line, sizeof(line), "O,thr,%d,servo", fastTCached);
        testSerial.println(line);
    }

    snprintf(line, sizeof(line), "O,done,1,%ld,%ld,%ld,%ld,%ld,%d,%lu,\"ok\"",
        (long) home, (long) lead, (long) trail, (long) switchSize,
        (long) backlash, T, (unsigned long) (millis() - t0));
    testSerial.println(line);
}

// ---------------------------------------------------------------------------
// Full-rev sensor census at a fixed threshold (command N)
// ---------------------------------------------------------------------------
// One ramped forward lap (one rev + 2%) with the debounced ISR chain latch
// recording every transition - a microstep-resolution map of every feature the
// sensor sees at threshold T. Restores the operating DAC afterwards; ends one
// rev forward of where it started (same physical angle).
static void censusRun(int T, StepsPerSecond vmax, StepsPerSecondPerSecond accel,
                      uint16_t m) {
    const Steps rev = motion->getMicrostepsPerPrismRotation();
    const Steps lap = rev + rev / 50;
    char line[96];

    motion->setLatchDebounce(m);
    motion->enable(true);

    snprintf(line, sizeof(line), "N,begin,%ld,%d,%ld,%ld",
        (long) motion->getPosition(), T, (long) vmax, (long) lap);
    testSerial.println(line);

    Modules::HomeSwitchOptical::setThreshold((uint8_t) T);
    if (!waitServiced(kFastSettleMs)) {
        Modules::HomeSwitchOptical::setThreshold(threshold);
        motion->clearAbort();
        testSerial.println("N,end,0,1");
        return;
    }

    bool ok;
    int n = motion->censusLap(vmax, accel, lap, censusEdges, censusEdgeStates,
        (int) (sizeof(censusEdges) / sizeof(censusEdges[0])), ok);

    for (int i = 0; i < n; i++) {
        snprintf(line, sizeof(line), "N,edge,%d,%ld,%d",
            i, (long) censusEdges[i], (int) censusEdgeStates[i]);
        testSerial.println(line);
    }

    Modules::HomeSwitchOptical::setThreshold(threshold);
    motion->clearAbort();
    snprintf(line, sizeof(line), "N,end,%d,%d", n, ok ? 0 : 1);
    testSerial.println(line);
}

// ---------------------------------------------------------------------------
// OLED
// ---------------------------------------------------------------------------
static void drawOled() {
    if (!oledOk) return;
    u8g2.clearBuffer();
    u8g2.setFont(u8g2_font_5x7_tr);
    const char * mstr = (mode == MODE_SWEEP) ? "SWEEP"
                      : (mode == MODE_TRACK) ? "TRACK" : "IDLE";
    char buf[24];
    snprintf(buf, sizeof(buf), "BENCH-A %s", mstr);
    u8g2.drawStr(0, 7, buf);

    // Sensor state box.
    bool level = motion->sensorActive();
    if (level) { u8g2.drawBox(0, 12, 40, 24); u8g2.setDrawColor(0); }
    u8g2.drawFrame(0, 12, 40, 24);
    u8g2.setFont(u8g2_font_6x12_mr);
    u8g2.drawStr(6, 29, level ? "FLAG" : " -- ");
    u8g2.setDrawColor(1);

    u8g2.setFont(u8g2_font_5x7_tr);
    snprintf(buf, sizeof(buf), "thr %d", (int) threshold);
    u8g2.drawStr(46, 20, buf);
    long degx10 = (long) lroundf(3600.0f * (float) motion->getPosition()
        / (float) motion->getMicrostepsPerPrismRotation());
    snprintf(buf, sizeof(buf), "%ld.%01ld deg", degx10 / 10, labs(degx10 % 10));
    u8g2.drawStr(46, 30, buf);

    snprintf(buf, sizeof(buf), "pos %ld", (long) motion->getPosition());
    u8g2.drawStr(0, 46, buf);
    snprintf(buf, sizeof(buf), "%s %s %s",
        motion->getEnabled() ? "EN" : "--",
        motion->getRunning() ? "RUN" : "   ",
        homed ? "HOMED" : "");
    u8g2.drawStr(0, 57, buf);
    if (motion->getFault()) u8g2.drawStr(100, 57, "FLT");

    u8g2.sendBuffer();
}

// ---------------------------------------------------------------------------
void setup() {
    SystemClock_Config();

    pinMode(PB3, OUTPUT);   // LED: sensor A level
    pinMode(PB4, OUTPUT);   // LED: motor running

    testSerial.begin(115200);
    testSerial.println();
    testSerial.println("# HomeSwitchTest bench - Side A: sensor + threshold + motor + homing");
    testSerial.println("# cmds: T M E J G V H Z X P R C Q F K A O N U D Y  (see bench_main.cpp header)");

    if (u8x8_stm32_init_i2c()) {
        u8g2_Setup_ssd1306_i2c_128x64_noname_f(u8g2.getU8g2(), U8G2_R2,
            u8x8_byte_stm32_hw_i2c, u8x8_stm32_gpio_and_delay);
        u8x8_SetI2CAddress(u8g2.getU8x8(), 0x3c);
        u8g2.begin();
        oledOk = true;
    } else {
        testSerial.println("# WARNING: OLED not detected");
    }

    sensorA = new Modules::HomeSwitchOptical(Modules::HomeSwitchOptical::Config::A());
    sensorA->setup();   // starts the shared TIM6 software-PWM threshold generator

    motorSettings = new Modules::MotorDriverSettings(Modules::MotorDriverSettings::Config());
    // Aug 2026: back on side A (32:1) for the injection-moulded ring gear tests.
    // (The Jul 2026 16:1 module was assembled on the B motor + B sensor channels.)
    motorDriverA = new Modules::MotorDriver(Modules::MotorDriver::Config::MotorA());
    motion = new Bench::Motion(*motorSettings, *motorDriverA, *sensorA);
    motion->setup();
    motion->setServiceCallback(serviceTick);

    Modules::HomeSwitchOptical::setThreshold(threshold);

    emitBanner();
}

void loop() {
    pumpSerial();

    switch (mode) {
        case MODE_SWEEP:
            doSweepA();
            break;
        case MODE_TRACK:
        case MODE_IDLE:
        default:
            emitStatusThrottled();
            break;
    }

    digitalWrite(PB3, motion->sensorActive() ? HIGH : LOW);
    digitalWrite(PB4, motion->getRunning() ? HIGH : LOW);

    // Idle driver sleep (see driverWake above).
    if (busy || motion->getRunning()) {
        lastActiveMs = millis();
    } else if (motion->getEnabled() && !driverAsleep
               && millis() - lastActiveMs > kIdleSleepMs) {
        motorSettings->setSleep(true);
        driverAsleep = true;
    }

    // Skip the OLED (its full-frame I2C write blocks ~90 ms) while the motor is
    // running, so the loop stays fast: crisp jog-dial stop and ~60 Hz status.
    static uint32_t lastOledMs = 0;
    uint32_t now = millis();
    if (!motion->getRunning() && now - lastOledMs >= kOledPeriodMs) {
        lastOledMs = now;
        drawOled();
    }
}
