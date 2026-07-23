#include "BenchMotion.h"

#include <Arduino.h>
#include <math.h>

namespace Bench {

	//----------
	Motion::Motion(Modules::MotorDriverSettings& motorDriverSettings
		, Modules::MotorDriver& motorDriver
		, Modules::HomeSwitchOptical& homeSwitch)
	: motorDriverSettings(motorDriverSettings)
	, motorDriver(motorDriver)
	, homeSwitch(homeSwitch)
	{
	}

	//----------
	void
	Motion::setup()
	{
		// Push safe default driver settings: driver awake, coils off, 32
		// microsteps, default current.
		this->motorDriverSettings.setMicrostepResolution(Modules::MotorDriverSettings::MicrostepResolution::_32);
		this->motorDriverSettings.setCurrent(MOTORDRIVERSETTINGS_DEFAULT_CURRENT);
		this->motorDriverSettings.setSleep(false);
		this->motorDriver.setEnabled(false);
		this->enabled = false;

		// Build the step timer (mirrors MotionControl::initTimer).
		auto stepPin = this->motorDriver.getConfig().StepTimerPin;
		this->timer.instance = (TIM_TypeDef*) pinmap_peripheral(stepPin, PinMap_TIM);
		this->timer.hardwareTimer = new HardwareTimer(this->timer.instance);
		this->timer.channel = STM_PIN_CHANNEL(pinmap_function(stepPin, PinMap_TIM));

		this->timer.hardwareTimer->setMode(this->timer.channel
			, TIMER_OUTPUT_COMPARE_PWM1
			, stepPin);
		this->timer.hardwareTimer->setOverflow(1000, MICROSEC_FORMAT);
		this->timer.hardwareTimer->setCaptureCompare(this->timer.channel
			, 127
			, TimerCompareFormat_t::RESOLUTION_8B_COMPARE_FORMAT);
		this->timer.hardwareTimer->pause();

		this->timer.hardwareTimer->attachInterrupt([this]() {
			this->onStep();
		});

		// Cache the sensor GPIO port + pin mask for a direct IDR read in the
		// step ISR (digitalRead's pin-map lookup is too slow at high step rates).
		{
			PinName pinName = digitalPinToPinName(this->homeSwitch.getPinSensor());
			this->sensorPort = get_GPIO_Port(STM_PORT(pinName));
			this->sensorPinMask = STM_LL_GPIO_PIN(pinName);
		}
	}

	//----------
	void
	Motion::onStep()
	{
		// One pulse == one microstep. Track a signed position and, while armed,
		// latch the sensor reaching the wanted state for `latchDebounce`
		// CONSECUTIVE samples (noise-robust); sensorSeenPosition records the
		// M-th (confirming) sample - readers subtract (M-1) to report the first
		// sample of the run. Sensor is active-high (== getForwardsActive()).
		this->position += this->directionForward ? 1 : -1;

		const bool active = (this->sensorPort->IDR & this->sensorPinMask) != 0;

		if(this->armed && !this->sensorSeen) {
			if(active == this->latchWantActive) {
				if(++this->latchRunCount >= this->latchDebounce) {
					this->sensorSeen = true;
					this->sensorSeenPosition = this->position;
				}
			}
			else {
				this->latchRunCount = 0;
			}
		}
		else if(this->censusArmed) {
			// Chain latch: record every debounced level change.
			if(active != this->censusState) {
				if(++this->censusRunCount >= this->latchDebounce) {
					this->censusState = active;
					this->censusRunCount = 0;
					int n = this->censusCount;
					if(n < this->censusCapacity) {
						this->censusPositions[n] = this->position;
						this->censusStates[n] = active ? 1 : 0;
						this->censusCount = n + 1;
					}
				}
			}
			else {
				this->censusRunCount = 0;
			}
		}
	}

	//----------
	Steps
	Motion::getPosition() const
	{
		// int32_t read is atomic on Cortex-M0+.
		return this->position;
	}

	//----------
	bool
	Motion::getFault() const
	{
		// Fault line is active-low (open-drain from the driver IC).
		return digitalRead(this->motorDriver.getConfig().Fault) == LOW;
	}

	//----------
	Steps
	Motion::getMicrostepsPerStep() const
	{
		return this->motorDriverSettings.getMicrostepsPerStep();
	}

	//----------
	Steps
	Motion::getMicrostepsPerPrismRotation() const
	{
		// Exact rational, rounded: gearRatio*118*9759/(296*21) full steps per
		// prism rotation (32:1 -> 5928.247, 16:1 -> 2964.123). The truncated-
		// integer form (5928, as production's MOTION_STEPS_PER_PRISM_ROTATION)
		// is 7.9 microsteps/rev short at 32 microsteps - measurable as a
		// systematic homing shift after commanded full rotations.
		return microstepsPerPrismRotationFor(this->gearRatio, this->getMicrostepsPerStep());
	}

	//----------
	Steps
	Motion::microstepsPerPrismRotationFor(uint8_t ratio, Steps microstepsPerStep)
	{
		if (ratio == 16) {
			// The "16:1" modules do NOT follow the 32:1 rational with a halved
			// leading factor (nominal half = 94,852): bench-measured
			// 92,252 +/- 2 usteps/rev at 1/32 microstepping (Jul 2026, k-rev
			// homing ladder, k = 3..5). Implied true motor-gearbox ratio
			// ~15.562:1. Stored at 1/32 and scaled so other resolutions stay
			// consistent.
			return (Steps)((92252LL * (int64_t) microstepsPerStep + 16) / 32);
		}
		const int64_t num = (int64_t) ratio * 118LL * 9759LL * (int64_t) microstepsPerStep;
		const int64_t den = 296LL * 21LL;
		return (Steps) ((num + den / 2) / den);
	}

	//----------
	void
	Motion::setDefaultSpeed(StepsPerSecond speed)
	{
		if(speed < BENCH_MIN_SPEED) speed = BENCH_MIN_SPEED;
		if(speed > BENCH_MAX_SPEED) speed = BENCH_MAX_SPEED;
		this->defaultSpeed = speed;
	}

	//----------
	void
	Motion::enable(bool value)
	{
		this->enabled = value;
		this->motorDriver.setEnabled(value);
	}

	//----------
	void
	Motion::zero()
	{
		noInterrupts();
		this->position = 0;
		interrupts();
	}

	//----------
	void
	Motion::tick()
	{
		if(this->serviceTick) {
			this->serviceTick();
		}
	}

	//----------
	void
	Motion::clearLatch()
	{
		noInterrupts();
		this->sensorSeen = false;
		this->sensorSeenPosition = 0;
		this->latchRunCount = 0;
		interrupts();
	}

	//----------
	bool
	Motion::readLatch(Steps& positionOut)
	{
		noInterrupts();
		bool seen = this->sensorSeen;
		Steps p = this->sensorSeenPosition;
		bool dirForward = this->directionForward;
		uint16_t m = this->latchDebounce;
		interrupts();
		// The ISR latched the M-th (confirming) sample; report the FIRST sample
		// of the confirmed run - the true edge position.
		positionOut = dirForward ? p - (Steps)(m - 1) : p + (Steps)(m - 1);
		return seen;
	}

	//----------
	void
	Motion::shiftFrame(Steps home)
	{
		noInterrupts();
		this->position -= home;
		interrupts();
	}

	//----------
	void
	Motion::runAt(bool directionForward, StepsPerSecond speed)
	{
		if(!this->timer.hardwareTimer) {
			return;
		}
		if(speed < BENCH_MIN_SPEED) speed = BENCH_MIN_SPEED;
		if(speed > BENCH_MAX_SPEED) speed = BENCH_MAX_SPEED;

		this->enabled = true;
		this->motorDriver.setEnabled(true);

		this->directionForward = directionForward;
		this->motorDriver.setDirection(directionForward);

		this->timer.hardwareTimer->setOverflow((uint32_t) speed
			, TimerFormat_t::HERTZ_FORMAT);
		this->timer.hardwareTimer->setCaptureCompare(this->timer.channel
			, 127
			, TimerCompareFormat_t::RESOLUTION_8B_COMPARE_FORMAT);

		if(!this->running) {
			this->timer.hardwareTimer->resume();
			this->running = true;
		}
	}

	//----------
	void
	Motion::stop()
	{
		if(this->timer.hardwareTimer && this->running) {
			this->timer.hardwareTimer->pause();
			this->running = false;
		}
		// Motor is left enabled (holding torque). Use enable(false) to release.
	}

	//----------
	void
	Motion::goTo(Steps target, StepsPerSecond speed)
	{
		if(speed <= 0) speed = this->defaultSpeed;

		Steps start = this->getPosition();
		if(target == start) {
			return;
		}

		bool directionForward = target > start;
		this->runAt(directionForward, speed);

		while(true) {
			this->tick();

			if(this->abortRequested) {
				this->stop();
				return;
			}

			Steps p = this->getPosition();
			if(directionForward ? (p >= target) : (p <= target)) {
				this->stop();
				return;
			}

			delay(1);
		}
	}

	//----------
	void
	Motion::jog(Steps deltaMicrosteps, StepsPerSecond speed)
	{
		if(deltaMicrosteps == 0) {
			return;
		}
		this->goTo(this->getPosition() + deltaMicrosteps, speed);
	}

	//----------
	void
	Motion::runContinuous(StepsPerSecond signedSpeed)
	{
		this->clearAbort();
		if(signedSpeed == 0) {
			this->stop();
			return;
		}
		bool directionForward = signedSpeed > 0;
		StepsPerSecond speed = directionForward ? signedSpeed : -signedSpeed;
		this->runAt(directionForward, speed);   // non-blocking: starts the timer
	}

	//----------
	// Trapezoid speed profile, poll-driven: the same "reprogram the timer
	// overflow every frame" mechanism the production MotionControl uses. Speed
	// updates every >=1 ms; dt is clamped so a long service stall can't produce
	// a huge velocity jump.
	void
	Motion::goToRamped(Steps target, StepsPerSecond vmax
		, StepsPerSecondPerSecond accel)
	{
		if(vmax <= 0) vmax = this->defaultSpeed;
		if(vmax > BENCH_MAX_SPEED) vmax = BENCH_MAX_SPEED;
		if(accel <= 0) {
			this->goTo(target, vmax);
			return;
		}

		Steps start = this->getPosition();
		if(target == start) {
			return;
		}
		bool directionForward = target > start;

		StepsPerSecond v = BENCH_RAMP_FLOOR;
		uint32_t lastUpdate = micros();
		this->runAt(directionForward, v);

		while(true) {
			this->tick();

			if(this->abortRequested) {
				this->stop();
				return;
			}

			Steps p = this->getPosition();
			if(directionForward ? (p >= target) : (p <= target)) {
				this->stop();
				return;
			}

			uint32_t now = micros();
			uint32_t dt = now - lastUpdate;
			if(dt >= 1000) {
				lastUpdate = now;
				if(dt > 10000) dt = 10000;
				Steps remaining = directionForward ? (target - p) : (p - target);

				// Accelerate...
				StepsPerSecond vNew = v
					+ (StepsPerSecond)(((int64_t) accel * (int64_t) dt) / 1000000);
				if(vNew > vmax) vNew = vmax;
				// ...but respect the deceleration triangle into the target.
				float vDecel = sqrtf(2.0f * (float) accel * (float) remaining);
				if((float) vNew > vDecel) vNew = (StepsPerSecond) vDecel;
				if(vNew < BENCH_RAMP_FLOOR) vNew = BENCH_RAMP_FLOOR;

				if(vNew != v) {
					v = vNew;
					this->runAt(directionForward, v);
				}
			}

			delay(1);
		}
	}

	//----------
	Steps
	Motion::seekLatch(bool directionForward, bool wantActive
		, StepsPerSecond vmax, StepsPerSecondPerSecond accel
		, Steps maxDistance, uint32_t deadlineMs, bool& ok)
	{
		if(vmax <= 0) vmax = this->defaultSpeed;
		if(vmax > BENCH_MAX_SPEED) vmax = BENCH_MAX_SPEED;
		if(accel <= 0) accel = BENCH_RAMP_ACCEL;

		const Steps start = this->getPosition();
		this->latchWantActive = wantActive;
		this->clearLatch();
		this->armed = true;

		StepsPerSecond v = BENCH_RAMP_FLOOR;
		uint32_t lastUpdate = micros();
		this->runAt(directionForward, v);

		Steps edge = start;
		bool found = false;
		bool fail = false;

		while(true) {
			this->tick();

			if(this->abortRequested || millis() > deadlineMs) {
				fail = true;
				break;
			}
			if(this->readLatch(edge)) {
				found = true;
				break;
			}
			{
				Steps p = this->getPosition();
				Steps travelled = directionForward ? (p - start) : (start - p);
				if(travelled >= maxDistance) {
					break;   // distance budget spent, nothing latched
				}
			}

			uint32_t now = micros();
			uint32_t dt = now - lastUpdate;
			if(dt >= 1000) {
				lastUpdate = now;
				if(dt > 10000) dt = 10000;
				StepsPerSecond vNew = v
					+ (StepsPerSecond)(((int64_t) accel * (int64_t) dt) / 1000000);
				if(vNew > vmax) vNew = vmax;
				if(vNew != v) {
					v = vNew;
					this->runAt(directionForward, v);
				}
			}

			delay(1);
		}
		this->armed = false;

		// Ramp down (2x accel) instead of stopping dead from speed - an abrupt
		// stop can shear steps and shift the counter frame more than necessary.
		while(v > BENCH_RAMP_FLOOR && !this->abortRequested) {
			this->tick();
			uint32_t now = micros();
			uint32_t dt = now - lastUpdate;
			if(dt >= 1000) {
				lastUpdate = now;
				if(dt > 10000) dt = 10000;
				v -= (StepsPerSecond)(((int64_t) accel * 2 * (int64_t) dt) / 1000000);
				if(v < BENCH_RAMP_FLOOR) v = BENCH_RAMP_FLOOR;
				this->runAt(directionForward, v);
			}
			delay(1);
		}
		this->stop();

		ok = found && !fail;
		return edge;
	}

	//----------
	int
	Motion::censusLap(StepsPerSecond vmax, StepsPerSecondPerSecond accel
		, Steps distance, Steps * positions, uint8_t * states
		, int capacity, bool& ok)
	{
		// Arm the chain latch seeded with the current raw level, then one ramped
		// forward run; the ISR records every debounced transition.
		noInterrupts();
		this->censusPositions = positions;
		this->censusStates = states;
		this->censusCapacity = capacity;
		this->censusCount = 0;
		this->censusRunCount = 0;
		this->censusState = (this->sensorPort->IDR & this->sensorPinMask) != 0;
		this->censusArmed = true;
		interrupts();

		this->goToRamped(this->getPosition() + distance, vmax, accel);

		noInterrupts();
		this->censusArmed = false;
		int n = this->censusCount;
		interrupts();

		// Report each edge at the FIRST sample of its confirmed run (forward lap).
		const Steps back = (Steps)(this->latchDebounce - 1);
		for(int i = 0; i < n; i++) {
			positions[i] -= back;
		}

		ok = !this->abortRequested;
		return n;
	}

	//----------
	bool
	Motion::moveUntilSensor(bool directionForward, bool wantActive
		, StepsPerSecond speed, bool guardEnabled, Steps guardPosition
		, uint32_t deadlineMs)
	{
		if(this->sensorActive() == wantActive) {
			return true;
		}

		this->runAt(directionForward, speed);

		while(this->sensorActive() != wantActive) {
			this->tick();

			if(this->abortRequested) {
				this->stop();
				return false;
			}
			if(millis() > deadlineMs) {
				this->stop();
				return false;
			}
			if(guardEnabled) {
				Steps p = this->getPosition();
				if(directionForward ? (p >= guardPosition) : (p <= guardPosition)) {
					this->stop();
					return false;
				}
			}

			delay(1);
		}

		this->stop();
		return true;
	}

	//----------
	Steps
	Motion::approachEdge(bool directionForward, bool wantActive
		, StepsPerSecond speed, uint32_t deadlineMs, bool& ok)
	{
		this->latchWantActive = wantActive;
		this->clearLatch();
		this->armed = true;
		this->runAt(directionForward, speed);

		Steps edge = this->getPosition();
		while(true) {
			this->tick();

			if(this->abortRequested) {
				this->stop();
				this->armed = false;
				ok = false;
				return this->getPosition();
			}
			if(millis() > deadlineMs) {
				this->stop();
				this->armed = false;
				ok = false;
				return this->getPosition();
			}
			if(this->readLatch(edge)) {
				this->stop();
				this->armed = false;
				ok = true;
				return edge;
			}

			delay(1);
		}
	}

	//----------
	// Homing on the single optical flag. Both flag edges are latched in ONE
	// forward pass (leading edge on the rising transition, trailing edge on the
	// falling transition), so they share the same forward-engaged frame and the
	// midpoint is free of gear backlash. (The production MotionControl approaches
	// each edge from its own side but cancels backlash in the ISR; this bench
	// controller has no backlash model, so a single-direction sweep is the
	// simplest way to keep the two edges in one frame.)
	Motion::HomeResult
	Motion::homeSideA(StepsPerSecond slowSpeed, uint32_t timeoutMs)
	{
		HomeResult r;
		if(!this->timer.hardwareTimer) {
			r.message = "no timer";
			return r;
		}
		if(slowSpeed <= 0) slowSpeed = BENCH_SLOW_SPEED;

		this->clearAbort();
		this->enable(true);

		const Steps clearance = BENCH_CLEAR_SWITCH_STEPS * this->getMicrostepsPerStep();
		const Steps twoRotations = 2 * this->getMicrostepsPerPrismRotation();
		const uint32_t deadline = millis() + timeoutMs;

		// Phase 0: end up just below the flag's leading (lower) edge, off the flag.
		if(this->sensorActive()) {
			// On the flag: exit backward through the leading edge.
			if(!this->moveUntilSensor(false, false, slowSpeed, false, 0, deadline)) {
				r.message = this->aborted() ? "abort" : "timeout clearing flag";
				return r;
			}
		}
		else {
			// Not on the flag: search forward (bounded to two rotations)...
			Steps guard = this->getPosition() + twoRotations;
			if(!this->moveUntilSensor(true, true, this->defaultSpeed, true, guard, deadline)) {
				r.message = this->aborted() ? "abort" : "flag not found in 2 rotations";
				return r;
			}
			// ...then exit backward through the leading edge.
			if(!this->moveUntilSensor(false, false, slowSpeed, false, 0, deadline)) {
				r.message = this->aborted() ? "abort" : "timeout clearing flag";
				return r;
			}
		}

		// Back off a clearance (still backwards) so the forward sweep takes up the
		// backlash and settles before it reaches the leading edge.
		this->goTo(this->getPosition() - clearance, this->defaultSpeed);
		if(this->abortRequested) { r.message = "abort"; return r; }
		if(millis() > deadline)   { r.message = "timeout"; return r; }

		// Phase 1: slow forward until the sensor goes ACTIVE -> leading edge.
		bool ok1;
		Steps leadingEdge = this->approachEdge(true, true, slowSpeed, deadline, ok1);
		if(!ok1) {
			r.message = this->aborted() ? "abort" : "timeout on leading edge";
			return r;
		}

		// Phase 2: keep going forward (no reversal) until the sensor goes
		// INACTIVE -> trailing edge. Same engagement frame as the leading edge.
		bool ok2;
		Steps trailingEdge = this->approachEdge(true, false, slowSpeed, deadline, ok2);
		if(!ok2) {
			r.message = this->aborted() ? "abort" : "timeout on trailing edge";
			return r;
		}

		// Home is the flag centre. Re-label the frame (no move -> no backlash),
		// then park at the centre for a visual check.
		Steps home = (leadingEdge + trailingEdge) / 2;
		noInterrupts();
		this->position -= home;
		interrupts();

		this->goTo(0, this->defaultSpeed);   // convenience park (backlash-affected)

		r.ok = true;
		r.home = home;
		r.switchSize = trailingEdge - leadingEdge;
		r.leadingEdge = leadingEdge;
		r.trailingEdge = trailingEdge;
		r.message = "ok";
		return r;
	}
}
