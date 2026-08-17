// Lightweight, self-contained single-axis (Side A) stepper controller for the
// optical home-switch bench rig.
//
// It reuses the REAL PortalFW driver classes (Modules::MotorDriver,
// Modules::MotorDriverSettings, Modules::HomeSwitchOptical) but deliberately
// does NOT compile Modules::MotionControl - that class is coupled to App::X(),
// Logger and the keyframe subsystem. Instead this mirrors the parts of
// MotionControl we need (timer/ISR stepping, run/stop, blocking moves and the
// two-edge homing routine) in a form that has no dependency on the rest of the
// application.
//
// Geometry, clearance and speed constants are kept in sync with
// PortalFW/src/Modules/MotionControl.h (referenced inline below).

#pragma once

#include <stdint.h>
#include <functional>

#include "HardwareTimer.h"

#include "Modules/MotorDriver.h"
#include "Modules/MotorDriverSettings.h"
#include "Modules/HomeSwitchOptical.h"
#include "Modules/Types.h"

namespace Bench {

	// --- Constants mirrored from MotionControl.h (kept in sync by hand) ---------
	// One full prism/ring rotation is gearRatio * 118 * 9759 / (296 * 21) full
	// steps (32:1 modules -> 189,704 microsteps at 32 microsteps/step, 16:1
	// modules -> 94,852). The gear ratio is a runtime property of Motion (see
	// setGearRatio) because the two module generations differ only in that
	// leading factor.

	// Distance to clear a switch before a slow re-approach (full steps).
	// == MOTION_CLEAR_SWITCH_STEPS (20000 / 128).
	static const Steps BENCH_CLEAR_SWITCH_STEPS = (Steps)(20000 / 128);

	// Speeds in (micro)steps/second. == MOTION_DEFAULT_SPEED / slowMoveSpeed /
	// MOTION_MAX_SPEED / MotionProfile::minimumSpeed.
	static const StepsPerSecond BENCH_DEFAULT_SPEED = 7040 * 2;
	static const StepsPerSecond BENCH_SLOW_SPEED    = 2000;
	static const StepsPerSecond BENCH_MAX_SPEED     = 80000;
	static const StepsPerSecond BENCH_MIN_SPEED     = 5;

	// Ramped (trapezoid) moves. The floor keeps the step timer above ~1 kHz so
	// the stm32duino overflow reprogram never changes the prescaler mid-run
	// (PSC swaps below ~977 Hz can glitch one period), and it doubles as the
	// stepper's safe pull-in speed for a standing start.
	static const StepsPerSecond          BENCH_RAMP_FLOOR = 1000;
	static const StepsPerSecondPerSecond BENCH_RAMP_ACCEL = 100000;   // default; tuned empirically

	class Motion {
	public:
		struct HomeResult {
			bool  ok = false;
			Steps home = 0;        // flag centre, in the pre-zero frame
			Steps switchSize = 0;  // trailing - leading edge
			Steps leadingEdge = 0;
			Steps trailingEdge = 0;
			const char * message = "";
		};

		Motion(Modules::MotorDriverSettings&
			, Modules::MotorDriver&
			, Modules::HomeSwitchOptical&);

		// Create the step timer + interrupt and push default driver settings.
		void setup();

		// Called from bench_main between motion poll iterations: it should pump
		// the serial command parser (so X/abort still works) and emit telemetry.
		void setServiceCallback(std::function<void()> fn) { this->serviceTick = fn; }

		// --- State -----------------------------------------------------------
		Steps getPosition() const;
		bool  getRunning() const { return this->running; }
		bool  getEnabled() const { return this->enabled; }
		bool  sensorActive() const { return this->homeSwitch.getForwardsActive(); }
		// The ISR's direct register read of the same pin (diagnostics).
		bool  sensorActiveRaw() const {
			return this->sensorPort && (this->sensorPort->IDR & this->sensorPinMask) != 0;
		}
		uint32_t getSensorPinMask() const { return this->sensorPinMask; }
		bool  getFault() const;
		Steps getMicrostepsPerStep() const;
		Steps getMicrostepsPerPrismRotation() const;

		// The steps-per-rotation rational for an arbitrary ratio (used by the
		// fast-home motor detection to classify a measured revolution).
		static Steps microstepsPerPrismRotationFor(uint8_t ratio, Steps microstepsPerStep);

		// Output gear ratio of the attached module (32 or 16). Feeds the
		// steps-per-rotation rational; set by fast home's motor detection or the
		// U command. Defaults to 32 (the original modules).
		void    setGearRatio(uint8_t ratio) { if (ratio == 16 || ratio == 32) this->gearRatio = ratio; }
		uint8_t getGearRatio() const { return this->gearRatio; }

		void setDefaultSpeed(StepsPerSecond speed);
		StepsPerSecond getDefaultSpeed() const { return this->defaultSpeed; }

		// --- Actuation -------------------------------------------------------
		void enable(bool);              // motor coils on/off (holding torque)
		void zero();                    // set current position to 0
		void requestAbort() { this->abortRequested = true; }
		void clearAbort()   { this->abortRequested = false; }
		bool aborted() const { return this->abortRequested; }

		// Blocking moves (constant speed). They call serviceTick() each poll and
		// bail out early on abort. speed <= 0 means "use defaultSpeed".
		void jog(Steps deltaMicrosteps, StepsPerSecond speed);
		void goTo(Steps targetMicrosteps, StepsPerSecond speed);

		// Non-blocking continuous run for the jog dial: signed speed sets the
		// direction, 0 stops. The motor keeps running (the ISR advances position)
		// until runContinuous(0) / stop() — the main loop stays free to stream
		// status and accept the stop command.
		void runContinuous(StepsPerSecond signedSpeed);

		// Two-edge homing on the single optical flag (see .cpp for the sequence).
		HomeResult homeSideA(StepsPerSecond slowSpeed, uint32_t timeoutMs);

		// Slow armed approach in a fixed direction; returns the position at which the
		// sensor first reaches `wantActive` (a rising or falling edge). ok=false on
		// abort / timeout. Public so bench_main's autoHome can orchestrate its own
		// two-edge pass at a self-calibrated threshold.
		Steps approachEdge(bool directionForward, bool wantActive
			, StepsPerSecond speed, uint32_t deadlineMs, bool& ok);

		// Coarse move until the sensor reaches wantActive, optionally bounded by
		// a guard position (pass guardEnabled=false to ignore it). Returns false
		// on abort / timeout / guard overrun. Public so bench_main's routines can
		// orchestrate their own sequences.
		bool moveUntilSensor(bool directionForward, bool wantActive
			, StepsPerSecond speed, bool guardEnabled, Steps guardPosition
			, uint32_t deadlineMs);

		// Blocking trapezoid move: ramp up at `accel` toward `vmax`, ramp down into
		// the target. Same poll/abort semantics as goTo. accel<=0 falls back to a
		// constant-speed goTo.
		void goToRamped(Steps targetMicrosteps, StepsPerSecond vmax
			, StepsPerSecondPerSecond accel);

		// Accelerated armed seek: ramp toward vmax with the debounced latch armed;
		// on latch (or abort / deadline / maxDistance travelled) decelerate and
		// stop. Returns the latched edge position; ok=false if nothing latched.
		// Step loss at speed only shifts the COUNTER frame - callers must treat the
		// result as coarse and re-reference with a slow armed pass.
		Steps seekLatch(bool directionForward, bool wantActive
			, StepsPerSecond vmax, StepsPerSecondPerSecond accel
			, Steps maxDistance, uint32_t deadlineMs, bool& ok);

		// Edge-capture debounce: the ISR latch fires on the M-th consecutive
		// µstep-sample in the wanted state; readers report the position of the
		// FIRST sample of that run. M<=1 = legacy single-sample latch.
		void setLatchDebounce(uint16_t m) { this->latchDebounce = (m < 1) ? 1 : m; }
		uint16_t getLatchDebounce() const { return this->latchDebounce; }

		// Census: one ramped forward run of `distance` µsteps recording every
		// debounced sensor transition (chain latch) into positions/states
		// (state = the NEW debounced level, position = first sample of its run).
		// Returns the number of edges recorded (clamped to capacity); ok=false on
		// abort.
		int censusLap(StepsPerSecond vmax, StepsPerSecondPerSecond accel
			, Steps distance, Steps * positions, uint8_t * states
			, int capacity, bool& ok);

		// Shift the coordinate frame so that `home` becomes 0, without moving
		// (exact - no park-overrun error contaminates the datum).
		void shiftFrame(Steps home);

		// Timer ISR entry point (public so the attached lambda can call it).
		void onStep();

	private:
		Modules::MotorDriverSettings& motorDriverSettings;
		Modules::MotorDriver&         motorDriver;
		Modules::HomeSwitchOptical&   homeSwitch;

		struct {
			TIM_TypeDef*    instance = nullptr;
			HardwareTimer*  hardwareTimer = nullptr;
			uint32_t        channel = 0;
		} timer;

		StepsPerSecond defaultSpeed = BENCH_DEFAULT_SPEED;
		uint8_t gearRatio = 32;
		bool enabled = false;
		bool running = false;

		// Written by the ISR, read by the main loop. int32_t is single-word
		// aligned so reads are atomic on Cortex-M0+.
		volatile Steps position = 0;
		volatile bool  directionForward = true;

		volatile bool  armed = false;         // latch a sensor transition while true
		volatile bool  latchWantActive = true; // the sensor state to latch on
		volatile bool  sensorSeen = false;
		volatile Steps sensorSeenPosition = 0;   // position of the M-th (confirming) sample

		// Debounced latch: require M consecutive matching samples (see
		// setLatchDebounce). runCount is ISR-private.
		volatile uint16_t latchDebounce = 1;
		volatile uint16_t latchRunCount = 0;

		// Census chain latch (see censusLap): while censusArmed, the ISR records
		// every debounced level change into the caller-provided buffers.
		volatile bool     censusArmed = false;
		volatile bool     censusState = false;   // current debounced level
		volatile uint16_t censusRunCount = 0;
		Steps *           censusPositions = nullptr;
		uint8_t *         censusStates = nullptr;
		volatile int      censusCount = 0;
		int               censusCapacity = 0;

		// Direct register-read cache for the sensor pin (ISR-safe; Arduino
		// digitalRead costs ~1-1.5 µs which is real CPU at high step rates).
		GPIO_TypeDef * sensorPort = nullptr;
		uint32_t       sensorPinMask = 0;

		volatile bool  abortRequested = false;

		std::function<void()> serviceTick;

		void runAt(bool directionForward, StepsPerSecond speed);
		void stop();
		void tick();                          // serviceTick() if set
		void clearLatch();
		bool readLatch(Steps& positionOut);   // atomically read the edge latch
	};
}
