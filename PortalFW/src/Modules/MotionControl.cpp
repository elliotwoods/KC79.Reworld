#include "MotionControl.h"
#include "Logger.h"
#include "App.h"

namespace Modules {
	//----------
	bool
	MotionControl::HealthStatus::allOK() const
	{
		return this->measureCycleOK
			&& this->switchesOK
			&& this->backlashOK
			&& this->homeOK;
	}

	//----------
	MotionControl::MotionControl(MotorDriverSettings& motorDriverSettings
		, MotorDriver& motorDriver
		, HomeSwitch& homeSwitch)
	: motorDriverSettings(motorDriverSettings)
	, motorDriver(motorDriver)
	, homeSwitch(homeSwitch)
	{
		// Set the name to the axis label
		sprintf(this->name, "MotionControl_%c", motorDriver.getConfig().AxisLabel);

		this->initTimer();
	}

	//----------
	bool
	MotionControl::readMeasureRoutineSettings(Stream& stream, MeasureRoutineSettings& settings)
	{
		// Expecting Nil or array of arguments
		msgpack::DataType dataType;
		if(!msgpack::getNextDataType(stream, dataType)) {
			return false;
		}

		if(dataType == msgpack::DataType::Nil) {
			msgpack::readNil(stream);
		}
		else if(dataType == msgpack::DataType::Array) {
			size_t arraySize;
			msgpack::readArraySize(stream, arraySize);

			if(arraySize >= 1) {
				int32_t value;
				if(!msgpack::readInt<int32_t>(stream, value)) {
					return false;
				}
				settings.timeout_s = (uint8_t) value;
			}
			if(arraySize >= 2) {
				if(!msgpack::readInt<int32_t>(stream, settings.slowMoveSpeed)) {
					return false;
				}
			}
			if(arraySize >= 3) {
				if(!msgpack::readInt<int32_t>(stream, settings.backOffDistance)) {
					return false;
				}
			}
			if(arraySize >= 4) {
				if(!msgpack::readInt<int32_t>(stream, settings.debounceDistance)) {
					return false;
				}
			}
			if(arraySize >= 5) {
				if(!msgpack::readInt<uint8_t>(stream, settings.tryCount)) {
					return false;
				}
			}
		}
		else {
			return false;
		}

		return true;
	}

	//----------
	const char *
	MotionControl::getTypeName() const
	{
		return "MotionControl";
	}

	//----------
	const char *
	MotionControl::getName() const
	{
		return this->name;
	}

	//----------
	void
	MotionControl::update()
	{
		this->updateStepsAndSwitches();

		if(this->motionProfile.maximumSpeed > MOTION_MAX_SPEED) {
			this->motionProfile.maximumSpeed = MOTION_MAX_SPEED;
		}

		this->updateFilteredMotion();

		this->updateMotion();
	}

	//----------
	const MotionControl::FrameSwitchEvents &
	MotionControl::getFrameSwitchEvents() const
	{
		return this->frameSwitchEvents;
	}

	//----------
	void
	MotionControl::testTimer()
	{
		uint32_t target_count = 50000;
		uint32_t period_us = 100;
		
		int currentCount = 0;

		this->initTimer();
		this->motorDriver.setEnabled(true);

		this->disableInterrupt();

		this->timer.hardwareTimer->attachInterrupt([&]() {
			currentCount++;
		});

		this->motorDriver.setEnabled(true);
		this->timer.hardwareTimer->resume();

		// create moduleName
		char moduleName[100];
		sprintf(moduleName, "%s.testTimer", this->getName());

		log(LogLevel::Status, moduleName, "Test begin");

		do {
			HAL_Delay(10);

			// Print message
			{
				char message[100];
				sprintf(message, "%d->%d (%d)\n"
					, (int) currentCount
					, (int) target_count
					, (int) period_us);
				log(LogLevel::Status, moduleName, message);
			}
		} while (currentCount < target_count);

		log(LogLevel::Status, moduleName, "Test end");
		// this->timer.hardwareTimer->pause();

		// // destroy it for now (so that we can call multiple times)
		// delete this->timer.hardwareTimer;

		this->deinitTimer();

		this->motorDriver.setEnabled(false);
	}

	//----------
	void
	MotionControl::initTimer()
	{
		if(this->timer.hardwareTimer) {
			this->deinitTimer();
		}

		auto stepPin = motorDriver.getConfig().StepTimerPin;
		auto timer = (TIM_TypeDef *) pinmap_peripheral(stepPin, PinMap_TIM);
		this->timer.hardwareTimer = new HardwareTimer(timer);
		this->timer.channel = STM_PIN_CHANNEL(pinmap_function(stepPin, PinMap_TIM));
		
		this->timer.hardwareTimer->setMode(this->timer.channel
			, TIMER_OUTPUT_COMPARE_PWM1
			, stepPin);
		
		this->timer.hardwareTimer->setOverflow(1000, MICROSEC_FORMAT);
		this->timer.hardwareTimer->setCaptureCompare(this->timer.channel
			, 127
			, TimerCompareFormat_t::RESOLUTION_8B_COMPARE_FORMAT);
		this->timer.hardwareTimer->pause();

		this->enableInterrupt();
	}

	//----------
	void
	MotionControl::deinitTimer()
	{
		this->disableInterrupt();
		this->timer.hardwareTimer->pause();
		delete this->timer.hardwareTimer;
		this->timer.hardwareTimer = nullptr;
	}

	//----------
	void
	MotionControl::disableInterrupt()
	{
		if(!this->interruptEnabled) {
			return;
		}

		if(!this->timer.hardwareTimer) {
			return;
		}

		this->timer.hardwareTimer->detachInterrupt();
		this->interruptEnabled = false;
	}

	//----------
	void
	MotionControl::enableInterrupt()
	{
		if(this->interruptEnabled) {
			return;
		}

		if(!this->timer.hardwareTimer) {
			return;
		}

		// Setup the interrupt
		this->timer.hardwareTimer->attachInterrupt([&]() {
			// This interrupt is called every time a step occurs

			auto& switchesSeen = this->inInterrupt.switchesSeen;

			this->inInterrupt.stepCount++;

			if(this->switchesArmed) {
				// Debounced over switchLatchDebounce consecutive µstep samples in the wanted
				// state, not a one-shot latch: the optical sensor's dip flanks are shallow
				// enough that comparator noise alone can dither the edge and latch a phantom
				// micro-flag (see HomeSwitchTest bench notes / PORTING.md). The reported
				// position is the FIRST sample of the confirmed run (subtract M-1 pulses,
				// clamped at 0 for a run that started within the first M pulses of this
				// frame) so both edges bias "late" by the same amount and the midpoint datum
				// stays clean. switchLatchDebounce stays 1 for HOME_SWITCH_LEGACY (mechanical
				// switch) builds, which makes this identical to the original one-shot latch:
				// with M=1, the run threshold fires on the very first matching sample and the
				// position offset is stepCount - 0.
				if(!switchesSeen.forwards.seen) {
					if (this->homeSwitch.getForwardsActive() ^ this->inInterrupt.invertSwitches) {
						if(++this->inInterrupt.fwRun >= this->switchLatchDebounce) {
							switchesSeen.forwards.seen = true;
							Steps debounceOffset = (Steps)(this->switchLatchDebounce - 1);
							switchesSeen.forwards.stepCountFirstSeen =
								this->inInterrupt.stepCount > debounceOffset
									? this->inInterrupt.stepCount - debounceOffset
									: 0;
						}
					} else {
						this->inInterrupt.fwRun = 0;
					}
				}

				if(!switchesSeen.backwards.seen) {
					if (this->homeSwitch.getBackwardsActive() ^ this->inInterrupt.invertSwitches) {
						if(++this->inInterrupt.bwRun >= this->switchLatchDebounce) {
							switchesSeen.backwards.seen = true;
							Steps debounceOffset = (Steps)(this->switchLatchDebounce - 1);
							switchesSeen.backwards.stepCountFirstSeen =
								this->inInterrupt.stepCount > debounceOffset
									? this->inInterrupt.stepCount - debounceOffset
									: 0;
						}
					} else {
						this->inInterrupt.bwRun = 0;
					}
				}
			}
			
		});

		this->interruptEnabled = true;
	}
	
	//----------
	Steps
	MotionControl::getPosition() const
	{
		return this->position;
	}

	//----------
	void
	MotionControl::setTargetPosition(Steps value)
	{
		this->targetPosition = value;

		// If anything sets the target position then reset the keyframeMotionControl system
		App::X().keyframeMotionControl->clear();
	}

	//----------
	Steps
	MotionControl::getTargetPosition() const
	{
		return this->targetPosition;
	}

	//----------
	Steps
	MotionControl::getClosestHomePosition() const
	{
		auto currentPosition = this->getPosition();
		auto currentCycle = currentPosition / (float) this->getMicrostepsPerPrismRotation();
		auto cycleRounded = round(currentCycle);
		auto closestHomePosition = (Steps) (cycleRounded * (float) this->getMicrostepsPerPrismRotation());
		return closestHomePosition;
	}

	//----------
	void
	MotionControl::setTargetPositionWithMotionFiltering(Steps value)
	{
		auto now = millis();

		// check that the messages are arriving frequently enough
		auto timeSinceLastPacket = now - this->motionFiltering.lastMoveMessageTime;
		if(timeSinceLastPacket > this->motionFiltering.allowedDuration) {
			// This is useful because a 'new movement' will need entirely new filtering
			this->motionFiltering.initialised = false;
		}

		if(!this->motionFiltering.initialised) {
			this->motionFiltering.active = false;
			this->motionFiltering.initialised = true;
		}
		else {
			this->motionFiltering.active = true;
			auto timeSinceLastMove = now - this->motionFiltering.lastMoveMessageTime;
			auto dsSinceLastMove = value - this->motionFiltering.lastPosition;
			this->motionFiltering.velocity = (Steps) dsSinceLastMove * 1000 / (Steps) timeSinceLastMove; // Hz
		}
		
		// We will always exit this function with initialised = true, so set these for next use
		this->motionFiltering.lastMoveMessageTime = now;
		this->motionFiltering.lastPosition = value;

		this->targetPosition = value;
	}

	//----------
	const MotionControl::MotionProfile &
	MotionControl::getMotionProfile() const
	{
		return this->motionProfile;
	}

	//----------
	void
	MotionControl::setMotionProfile(const MotionProfile& value)
	{
		this->motionProfile = value;
	}

	//----------
	bool
	MotionControl::getIsRunning() const
	{
		return this->timer.running;
	}

	//----------
	Steps
	MotionControl::getMicrostepsPerPrismRotation() const
	{
		auto microstepsPerStep = this->motorDriverSettings.getMicrostepsPerStep();

		// MOTION_STEPS_PER_PRISM_ROTATION expands to 32*118*9759/296/21 in left-to-right
		// integer math = 5928, but the exact ratio is 5928.247 full steps -- the truncated
		// constant is 7.9 microsteps/rev short at 32 microsteps/step (bench-measured as a
		// systematic shift after commanded full rotations; see HomeSwitchTest/portalfw_port
		// /PORTING.md). Homing is self-referencing so it still zeros correctly regardless,
		// but every position computed FROM this constant (multi-rev moves,
		// getClosestHomePosition, degree readouts) accumulates the truncation error, so
		// compute it as a single rounded rational instead of the compile-time macro.
		const int64_t num = (int64_t) MOTION_STEPS_PER_MOTOR_ROTATION
			* (int64_t) MOTION_GEAR_RING
			* 9759LL
			* (int64_t) microstepsPerStep;
		const int64_t den = 296LL * (int64_t) MOTION_GEAR_DRIVE;
		return (Steps)((num + den / 2) / den);
	}

	//----------
	void
	MotionControl::zeroCurrentPosition()
	{
		this->setCurrentPosition(0);
	}

	//----------
	void
	MotionControl::setCurrentPosition(Steps value)
	{
		this->position = value;
		this->targetPosition = value;
	}

	//----------
	void
	MotionControl::stop()
	{
		this->motorDriver.setEnabled(false);
		this->currentMotionState.motorRunning = false;

		if(this->timer.running && this->timer.hardwareTimer) {
			this->timer.hardwareTimer->pause();
			this->timer.running = false;
		}

		this->currentMotionState.speed = 0;
		this->targetPosition = this->getPosition();
	}

	//----------
	void
	MotionControl::run(bool direction, StepsPerSecond speed)
	{
		// If no hardware timer then nothing to do here
		if(!this->timer.hardwareTimer) {
			return;
		}

		// Check minimum speed
		if(speed < this->motionProfile.minimumSpeed) {
			speed = this->motionProfile.minimumSpeed;
		}

		// Run the motor
		this->motorDriver.setEnabled(true);
		this->currentMotionState.motorRunning = true;

		// Set the speed
		this->timer.hardwareTimer->setOverflow(speed
			, TimerFormat_t::HERTZ_FORMAT);
		this->currentMotionState.speed = speed;

		// Set 50% duty (always call this after setting speed)
		this->timer.hardwareTimer->setCaptureCompare(this->timer.channel
			, 127
			, TimerCompareFormat_t::RESOLUTION_8B_COMPARE_FORMAT);

		// Backlash control
		{
			if(direction && !this->currentMotionState.direction) {
				// Now going forwards, was going backwards before
				this->backlashControl.positionWithinBacklash -= this->backlashControl.systemBacklash;
			}
			else if(!direction && this->currentMotionState.direction) {
				// Now going backwards, was going forwards before
				this->backlashControl.positionWithinBacklash += this->backlashControl.systemBacklash;
			}
		}

		// Set the direction
		this->motorDriver.setDirection(direction);
		this->currentMotionState.direction = direction;

		// Start the timer (if paused)
		if(!this->timer.running) {
			this->timer.hardwareTimer->resume();
			this->timer.running = true;
		}
	}

	//----------
	bool
	MotionControl::processIncomingByKey(const char * key, Stream& stream)
	{
		if(strcmp("zeroCurrentPosition", key) == 0) {
			if(!msgpack::readNil(stream)) {
				return false;
			}
			this->zeroCurrentPosition();
			return true;
		}
		else if(strcmp("move", key) == 0) {
			msgpack::DataType dataType;
			if(!msgpack::getNextDataType(stream, dataType)) {
				return false;
			}

			// If it's just an int, then we take it as targetPosition
			if(dataType == msgpack::Int32
			|| dataType == msgpack::Int16
			|| dataType == msgpack::Int5
			|| dataType == msgpack::UInt32
			|| dataType == msgpack::UInt16
			|| dataType == msgpack::UInt8
			|| dataType == msgpack::UInt7) {
				return msgpack::readInt<int32_t>(stream, this->targetPosition);
			}
			else if(dataType == msgpack::Array) {
				size_t arraySize;
				if(!msgpack::readArraySize(stream, arraySize)) {
					return false;
				}

				// ARRAY FORMAT

				// POSITION
				if(arraySize >= 1) {
					if(!msgpack::readInt<int32_t>(stream, this->targetPosition)) {
						return false;
					}
				}

				// SPEED
				if(arraySize >= 2) {
					if(!msgpack::readInt<int32_t>(stream, this->motionProfile.maximumSpeed)) {
						return false;
					}
				}

				// ACCELERATION
				if(arraySize >= 3) {
					if(!msgpack::readInt<int32_t>(stream, this->motionProfile.acceleration)) {
						return false;
					}
				}

				// MIN SPEED
				if(arraySize >= 4) {
					if(!msgpack::readInt<int32_t>(stream, this->motionProfile.minimumSpeed)) {
						return false;
					}
				}

				return true;
			}
		}
		else if(strcmp(key, "motionProfile") == 0) {
			// MOTION PROFILE
			size_t arraySize;
			if(!msgpack::readArraySize(stream, arraySize)) {
				return false;
			}

			// ARRAY FORMAT

			// MAX SPEED
			if(arraySize >= 1) {
				if(!msgpack::readInt<int32_t>(stream, this->motionProfile.maximumSpeed)) {
					return false;
				}
			}

			// ACCELERATION
			if(arraySize >= 2) {
				if(!msgpack::readInt<int32_t>(stream, this->motionProfile.acceleration)) {
					return false;
				}
			}

			// MIN SPEED
			if(arraySize >= 3) {
				if(!msgpack::readInt<int32_t>(stream, this->motionProfile.minimumSpeed)) {
					return false;
				}
			}

			return true;
		}
		else if(strcmp(key, "unjam") == 0) {
			// UNJAM
			MeasureRoutineSettings settings;
			if(!MotionControl::readMeasureRoutineSettings(stream, settings)) {
				return false;
			}
			for(uint8_t i=0; i<settings.tryCount; i++) {
				auto exception = this->unjamRoutine(settings);
				if(exception) {
					log(exception);
				}
				else {
					return true;
				}
			}
			return false;
		}
		else if(strcmp(key, "tuneCurrent") == 0) {
			// TUNE CURRENT
			MeasureRoutineSettings settings;
			if(!MotionControl::readMeasureRoutineSettings(stream, settings)) {
				return false;
			}
			for(uint8_t i=0; i<settings.tryCount; i++) {
				auto exception = this->tuneCurrentRoutine(settings);
				if(exception) {
					log(exception);
				}
				else {
					return true;
				}
			}
			return false;
		}
		else if(strcmp(key, "measureBacklash") == 0) {
			// MEASURE BACKLASH
			MeasureRoutineSettings settings;
			if(!MotionControl::readMeasureRoutineSettings(stream, settings)) {
				return false;
			}
			for(uint8_t i=0; i<settings.tryCount; i++) {
				auto exception = this->measureBacklashRoutine(settings);
				if(exception) {
					log(exception);
				}
				else {
					return true;
				}
			}
			return false;
		}
		else if(strcmp(key, "home") == 0) {
			// HOMING ROUTINE
			MeasureRoutineSettings settings;
			if(!MotionControl::readMeasureRoutineSettings(stream, settings)) {
				return false;
			}
			for(uint8_t i=0; i<settings.tryCount; i++) {
				auto exception = this->homeRoutine(settings);
				if(exception) {
					log(exception);
				}
				else {
					return true;
				}
			}
			return false;
		}
		else if(strcmp(key, "initTimer") == 0) {
			if(!msgpack::readNil(stream)) {
				return false;
			}
			this->initTimer();
			return true;
		}
		else if(strcmp(key, "deinitTimer") == 0) {
			if(!msgpack::readNil(stream)) {
				return false;
			}
			this->deinitTimer();
			return true;
		}
		else if(strcmp(key, "testTimer") == 0) {
			if(!msgpack::readNil(stream)) {
				return false;
			}
			this->testTimer();
			return true;
		}
		else if(strcmp(key, "motionFilteringEnabled") == 0) {
			bool value;
			if(!msgpack::readBool(stream, value)) {
				return false;
			}
			this->motionFiltering.enabled = value;
		}
		return false;
	}

	//----------
	void
	MotionControl::updateStepsAndSwitches()
	{
		// Clear our outgoing event flags
		this->frameSwitchEvents.forwards.seen = false;
		this->frameSwitchEvents.backwards.seen = false;

		// Pull step count from interrupt
		auto stepCount = this->inInterrupt.stepCount;
		this->inInterrupt.stepCount = 0;
		
		// Can we do this fast?
		auto needsHandleSwitches = this->switchesArmed
			&& (this->inInterrupt.switchesSeen.forwards.seen
			 || this->inInterrupt.switchesSeen.backwards.seen);
		
		if(!needsHandleSwitches) {
			// Fast work
			if(this->currentMotionState.direction) {
				if(this->backlashControl.positionWithinBacklash < 0) {
					if(stepCount <= -this->backlashControl.positionWithinBacklash) {
						// still in backlash at end
						this->backlashControl.positionWithinBacklash += stepCount;
					}
					else {
						// out of backlash at end
						stepCount += this->backlashControl.positionWithinBacklash;
						this->backlashControl.positionWithinBacklash = 0;
						this->position += stepCount;
					}
				}
				else {
					this->position += stepCount;
				}
			}
			else {
				if(this->backlashControl.positionWithinBacklash > 0) {
					if(stepCount <= this->backlashControl.positionWithinBacklash) {
						// still in backlash at end
						this->backlashControl.positionWithinBacklash -= stepCount;
					}
					else {
						// out of backlash at end
						stepCount -= this->backlashControl.positionWithinBacklash;
						this->backlashControl.positionWithinBacklash = 0;
						this->position -= stepCount;
					}
				}
				else {
					this->position -= stepCount;
				}
			}
		}
		else {
			// Cycle through steps one by one (this is quite slow way but very clear what's happening)
			for(int i=0; i<stepCount; i++) {
				if(this->currentMotionState.direction) {
					// Forwards
					if(this->backlashControl.positionWithinBacklash < 0) {
						// Moving inside backlash region
						this->backlashControl.positionWithinBacklash++;
					}
					else {
						// Moving outside of backlash region
						this->position++;
					}
				}
				else {
					// Backwards
					if(this->backlashControl.positionWithinBacklash > 0) {
						// Moving inside backlash region
						this->backlashControl.positionWithinBacklash--;
					}
					else {
						// Moving outside of backlash region
						this->position--;
					}
				}

				// If on this step we saw the switch raise high
				if(!this->frameSwitchEvents.forwards.seen
					&& this->inInterrupt.switchesSeen.forwards.seen
					&& this->inInterrupt.switchesSeen.forwards.stepCountFirstSeen == i) {
					// Set our output flags for a switch seen event
					this->frameSwitchEvents.forwards.seen = true;
					this->frameSwitchEvents.forwards.positionSeen = this->position;
				}

				// If on this step we saw the switch raise high
				if(!this->frameSwitchEvents.backwards.seen
					&& this->inInterrupt.switchesSeen.backwards.seen
					&& this->inInterrupt.switchesSeen.backwards.stepCountFirstSeen == i) {
					// Set our output flags for a switch seen event
					this->frameSwitchEvents.backwards.seen = true;
					this->frameSwitchEvents.backwards.positionSeen = this->position;
				}
			}
		}

		

		// Clear the flags for interrupt so we get fresh events next frame
		this->inInterrupt.switchesSeen.forwards.seen = false;
		this->inInterrupt.switchesSeen.backwards.seen = false;
	}

	//----------
	void
	MotionControl::updateFilteredMotion()
	{
		if(this->motionFiltering.active) {

			const auto now = millis();
			const auto timeSinceLastMessage = now - this->motionFiltering.lastMoveMessageTime;
			
			// check if we've expired the motion filtering time window (stale data)
			if(timeSinceLastMessage > this->motionFiltering.allowedDuration) {
				this->motionFiltering.active = false;
				this->setTargetPosition(this->motionFiltering.lastPosition);
				return;
			}

			// calculate a target position based on motion filtering
			const auto targetPosition = this->motionFiltering.lastPosition
				+ (Steps) timeSinceLastMessage * this->motionFiltering.velocity / 1000;
			this->setTargetPosition(targetPosition);
		}
	}

	//----------
	// Remarkable August 2023 p1
	void
	MotionControl::updateMotion()
	{
		// Calculate a dt value in microseconds
		auto now = micros();
		auto dt = now - this->lastTime;
		this->lastTime = now;

		auto newMotionState = this->calculateMotionState(dt);

		if(!newMotionState.motorRunning && this->currentMotionState.motorRunning) {
			// Stop all motion
			this->stop();
		}

		if(newMotionState.motorRunning) {
			// Run the motion
			this->run(newMotionState.direction, newMotionState.speed);
		}

		// Print a debug message whilst moving
		if(this->currentMotionState.motorRunning) {
			// char message[64];
			// auto velocity = this->currentMotionState.direction
			// 	? this->currentMotionState.speed
			// 	: -this->currentMotionState.speed;
			// sprintf(message, "%d -> %d (%d)"
			// 	, this->position
			// 	, this->targetPosition
			// 	, velocity);
			// log(LogLevel::Status, message);
		}
	}

	//----------
	MotionControl::MotionState
	MotionControl::calculateMotionState(unsigned long dt_us) const
	{
		Steps deltaToTarget = this->targetPosition - this->position;
		Steps distanceToTarget = abs(deltaToTarget);

		// Check if we don't need to move
		// Warning : if we changed the targetPosition in the middle of a move then there's a risk that
		// we hit the targetPosition at a high velocity. The only solution to that is to re-home
		// We could also consider to hold the motor enabled high for Xms after.
		if(distanceToTarget == 0) {
			return MotionState {
				false
				, 0
				, true
			};
		}

		// We need to move

		bool directionToTarget = this->targetPosition - position > 0;
		// Cast through int64_t: at acceleration=100000 (the fast-home seek profile) this
		// multiplication overflows a 32-bit int for any dt_us above ~21 ms and corrupts the
		// ramp -- a real risk here since App::updateFromRoutine's blocking loops don't
		// guarantee a tight frame period.
		StepsPerSecond maxDeltaV = (StepsPerSecond)(
			(int64_t) this->motionProfile.acceleration * (int64_t) dt_us / 1000000);

		auto speed = this->currentMotionState.speed;

		// Do we need to change direction?
		if(this->currentMotionState.direction != directionToTarget) {
			if(this->currentMotionState.speed > 0) {
				// We're moving away from target, needs to accelerate towards target first
				
				speed -= maxDeltaV;

				if(speed < 0) {
					// decceleration resulted in direction change
					return MotionState {
						true
						, -speed
						, directionToTarget
					};
				}
				else if (speed < this->motionProfile.minimumSpeed) {
					// decellerated into minimum speed region and need to switch direction

					// switch to minimum speed in opposite direction
					return MotionState {
						true
						, this->motionProfile.minimumSpeed
						, directionToTarget
					};
				}
				else {
					// still moving in same direction, but deccelerating to change direction
					return MotionState {
						true
						, speed
						, this->currentMotionState.direction
					};
				}
			}
			// else we're not really moving at all, so just presume we're stationary and need to accelerate
			// therefore continue...
		}

		// We are moving towards target
		// Are we accelerating or deccelerating?
		bool needsDeccelerate = false;
		{
			// We're travelling towards target and may be close to the target and need to deccelerate, let's check

			// Calculate the time available in rest of motion profile if it's all in decceleration
			auto timeLeftInMotionProfile = (float) distanceToTarget * 2.0f / (float) speed;

			// Calculate time it would take to deccelerate to v=0
			auto timeItWouldTakeToDeccelerate = (float) speed / (float) this->motionProfile.acceleration;

			// Decide if we should be deccelerating
			if(timeLeftInMotionProfile <= timeItWouldTakeToDeccelerate) {
				needsDeccelerate = true;
			}
		}

		if(!needsDeccelerate) {
			// (1) - Accelerating or top speed

			if(speed < this->motionProfile.maximumSpeed) {
				// Accelerate
				speed += maxDeltaV;
			}

			// Cap speed
			if(speed > this->motionProfile.maximumSpeed) {
				speed = this->motionProfile.maximumSpeed;
			}
			
			return MotionState {
				true
				, speed
				, directionToTarget
			};
		}
		else {
			// (2) - Deccelerating

			// Deccelerate
			speed -= maxDeltaV;

			// Cap lowest speed
			if(speed < this->motionProfile.minimumSpeed) {
				speed = this->motionProfile.minimumSpeed;
			}

			return MotionState {
				true
				, speed
				, directionToTarget
			};
		}
	}

	//----------
	Exception
	MotionControl::unjamRoutine(const MeasureRoutineSettings& settings)
	{
#ifndef UNJAM_DISABLED
		// Stop any existing motion profile
		this->stop();

		// Create a string for the module name
		char moduleName[100];
		sprintf(moduleName, "%s.unjamRoutine", this->getName());

		if(!this->timer.hardwareTimer) {
			return Exception(moduleName, "No hardware timer");
		}

		log(LogLevel::Status, moduleName, "begin");

		// Store priors for later
		auto priorCurrent = this->motorDriverSettings.getCurrent();
		auto priorMicrostep = this->motorDriverSettings.getMicrostepResolution();
		auto priorMotionProfile = this->motionProfile;

		// We won't be calibrated any more after this
		this->healthStatus.homeOK = false;

		// We will check the switches
		this->switchesArmed = true;

		// Set the current to max and steps to 1
		this->motorDriverSettings.setCurrent(MOTORDRIVERSETTINGS_MAX_CURRENT);
		this->motorDriverSettings.setMicrostepResolution(MotorDriverSettings::MicrostepResolution::_1);

		// Tone down acceleration and velocity to match full steps movement
		MotionProfile unblockMotionProfile;
		{
			unblockMotionProfile.acceleration = 500;
			unblockMotionProfile.maximumSpeed = 523; // note C5
			unblockMotionProfile.minimumSpeed = 100;
		}
		this->setMotionProfile(unblockMotionProfile);

		// Start measuring time for timeout
		uint32_t startTime = millis();
		uint32_t routineDeadline = startTime + (uint32_t) settings.timeout_s * 1000U;

		// Create a function to handle ending of routine
		auto endRoutine = [&]() {
			this->stop();
			this->setMotionProfile(priorMotionProfile);
			this->motorDriverSettings.setCurrent(priorCurrent);
			this->motorDriverSettings.setMicrostepResolution(priorMicrostep);
			this->switchesArmed = false;
			log(LogLevel::Status, moduleName, "end");
		};
		
		// Set end position to be 1.5x complete rotation
		auto startPosition = this->getPosition();
		auto endPosition = startPosition + MOTION_STEPS_PER_PRISM_ROTATION * 4 / 3;
		this->setTargetPosition(endPosition);

		// Log if we ever see the switches (for safety sake)
		bool switchesSeen[2] { false, false};

		// Wait for move
		log(LogLevel::Status, moduleName, "Walk CW");
		{
			while (this->getPosition() < this->getTargetPosition())
			{
				this->update();
				if(App::updateFromRoutine()) { endRoutine(); return Exception::Escape(moduleName); }
				HAL_Delay(20); // longer delay because dt is otherwise too short for these steps

				if(this->frameSwitchEvents.forwards.seen && !switchesSeen[0]) {
					log(LogLevel::Status, moduleName, "FW switch seen");
					switchesSeen[0] = true;
				}

				if (millis() > routineDeadline)
				{
					endRoutine();
					return Exception::Timeout(moduleName);
				}
			}
		}

		if(!switchesSeen[0]) {
			endRoutine();
			return Exception(moduleName, "Didn't see FW switch");
		}

		// Instruct move back to 0
		this->setTargetPosition(startPosition);

		// Wait for move
		log(LogLevel::Status, moduleName, "Walk CCW");
		Steps backwardsSwitchSeenPosition;
		{
			while (this->getPosition() > this->getTargetPosition())
			{
				this->update();
				if(App::updateFromRoutine()) { endRoutine(); return Exception::Escape(moduleName); }
				HAL_Delay(20); // longer delay because dt is otherwise too short for these steps

				if(this->frameSwitchEvents.backwards.seen && !switchesSeen[1]) {
					log(LogLevel::Status, moduleName, "BW switch seen");
					switchesSeen[1] = true;
				}
				
				if (millis() > routineDeadline)
				{
					endRoutine();
					return Exception::Timeout(moduleName);
				}

				backwardsSwitchSeenPosition = this->frameSwitchEvents.backwards.positionSeen;
			}
		}

		if(!switchesSeen[1]) {
			endRoutine();
			return Exception(moduleName, "Didn't see BW switch");
		}

		endRoutine();

		this->healthStatus.switchesOK = true;

		// Make a guess for the home position
		{
			auto switchToHere = this->getPosition() - backwardsSwitchSeenPosition;

			// Take the position from this value and normalise it to the number of steps per rotation
			this->position = switchToHere * this->motorDriverSettings.getMicrostepsPerStep();
		}

#endif
		return Exception::None();
	}

	//----------
	Exception
	MotionControl::tuneCurrentRoutine(const MeasureRoutineSettings& settings)
	{
#ifndef TUNE_CURRENT_DISABLED
		// Create a module name
		char moduleName[100];
		sprintf(moduleName, "%s.tuneCurrentRoutine", this->getName());

		log(LogLevel::Status, moduleName, "begin");

		MotorDriverSettings::Amps current = MOTORDRIVERSETTINGS_DEFAULT_CURRENT;
		this->motorDriverSettings.setCurrent(current);

		this->switchesArmed = true;

		auto endRoutine = [&]() {
			this->switchesArmed = false;
			log(LogLevel::Status, moduleName, "end");
		};

		// Start measuring time for timeout
		uint32_t startTime = millis();
		uint32_t timeoutTime = startTime + (uint32_t) settings.timeout_s * 1000U;

		// Move off switch if already on
		if(this->homeSwitch.getForwardsActive()) {
			// We started on the switch

			// Set the target to be +1 rotation cycle
			this->setTargetPosition(this->getPosition() + this->getMicrostepsPerPrismRotation());

			log(LogLevel::Status, moduleName, "Move off switch");

			// Keep walking whilst we're on the switch
			while(this->homeSwitch.getForwardsActive()) {
				HAL_Delay(1);
				this->update();
				if(millis() > timeoutTime) { endRoutine(); return Exception::Timeout(moduleName); }
				if(App::updateFromRoutine()) { endRoutine(); return Exception::Escape(moduleName); }
			}

			// Stop once we're off the switch
			this->stop();
		}

		Steps forwardsSwitchPosition;

		while(true) {
			log(LogLevel::Status, moduleName, "Single rotation CW");
			
			// Try to move and see the switch
			auto startPosition = this->position;
			auto endPosition = startPosition + this->getMicrostepsPerPrismRotation() * 11 / 10;

			auto result = this->routineMoveToUntilSeeSwitch(endPosition
				, SwitchesMask { true, false}
				, timeoutTime);

			if(result.exception) {
				endRoutine();
				result.exception.setModuleName(moduleName);
				return result.exception;
			}

			if(result.frameSwitchEvents.forwards.seen) {
				log(LogLevel::Status, moduleName, "Switch seen");
				break;
			}
			else {
				// We didn't see the switch even though we performed enough steps
				current += 0.2f;
				if(current > MOTORDRIVERSETTINGS_MAX_CURRENT) {
					return Exception(moduleName, "Cannot raise the current higher");
				}
				else {
					{
						char message[100];
						sprintf(message, "Increasing current to %dmA", (int) (current * 1000.0f));
						log(LogLevel::Status, moduleName, message);
					}
					this->motorDriverSettings.setCurrent(current);
				}
			}
		}

		endRoutine();
#endif
		return Exception::None();
	}

	//----------
	// ReMarkable August 2023 page 2
	Exception
	MotionControl::measureBacklashRoutine(const MeasureRoutineSettings& settings)
	{
		// Create a module name
		char moduleName[100];
		sprintf(moduleName, "%s.measureBacklashRoutine", this->getName());

		if(!this->timer.hardwareTimer) {
			return Exception(moduleName, "No hardware timer");
		}

		// Stop any existing motion profile
		this->stop();

		const auto microStepsPerStep = this->motorDriverSettings.getMicrostepsPerStep();
		const auto microstepsPerPrismRotation = this->getMicrostepsPerPrismRotation();

		HAL_Delay(10);

		// Start measuring time for timeout
		uint32_t startTime = millis();
		uint32_t timeoutTime = startTime + (uint32_t) settings.timeout_s * 1000U;

		this->switchesArmed = true;

		// This will be used in the update cycle, so we want to clear it out whilst taking measurements
		this->backlashControl.systemBacklash = 0;

		log(LogLevel::Status, moduleName, "begin");

		auto endRoutine = [this, &moduleName]() {
			this->inInterrupt.invertSwitches = false; // we might invert during the routine
			this->stop();
			this->switchesArmed = false;
			log(LogLevel::Status, moduleName, "end");
		};

		// https://paper.dropbox.com/doc/KC79-Firmware-development-log--B9ww1dZ58Y0lrKt6fzBa9O8yAg-NaTWt2IkZT4ykJZeMERKP#:h2=Backlash-measure-algorithm
		Steps backlashSize;
		{
			Steps positionFWSwitchAccurate;
			log(LogLevel::Status, moduleName, "1: Find FW switch accurate");
			{
				// For a start we want to find the start of the forwards switch
				auto result = this->routineFindSwitchAccurate(true
					, settings.slowMoveSpeed
					, true
					, timeoutTime);

				if(result.exception) {
					endRoutine();
					result.exception.setModuleName(moduleName);
					return result.exception;
				}

				positionFWSwitchAccurate = result.frameSwitchEvents.forwards.positionSeen;
			}

			HAL_Delay(500);

			log(LogLevel::Status, moduleName, "2: Walk into switch (debounce)");
			{
				auto targetPosition = positionFWSwitchAccurate + settings.debounceDistance * microStepsPerStep;
				
				auto result = this->routineMoveTo(targetPosition, timeoutTime);

				if(result.exception) {
					endRoutine();
					result.exception.setModuleName(moduleName);
					return result.exception;
				}

				// Check that forwards is still active (in debounce)
				if(!this->homeSwitch.getForwardsActive()) {
					endRoutine();
					return Exception(moduleName, "Debounce error");
				}
			}

			log(LogLevel::Status, moduleName, "3: Back off to find backlash");
			Steps disengagePosition;
			{
				// Look for when switches drop low
				this->inInterrupt.invertSwitches = true;
				this->updateStepsAndSwitches(); // clear any positive events out

				MotionControl::RoutineMoveResult result;

				result = this->routineMoveToFindSwitch(false
					, settings.slowMoveSpeed
					, SwitchesMask { true, false }
					, timeoutTime);
				
				if(result.exception) {
					endRoutine();
					result.exception.setModuleName(moduleName);
					return result.exception;
				}

				disengagePosition = result.frameSwitchEvents.forwards.positionSeen;

				this->inInterrupt.invertSwitches = false;
			}

			backlashSize = positionFWSwitchAccurate - disengagePosition;
		}
			
		// Measure the current position at end of sequence
		{
			auto backlashInDegrees = 360.0f * (float) backlashSize / (float) microstepsPerPrismRotation;
			char message[100];
			sprintf(message
				, "Backlash = %d (%d/10 degrees)"
				, backlashSize
				, (int) (backlashInDegrees * 10));
			log(LogLevel::Status, moduleName, message);
		}

		if(backlashSize > 0) {
			this->backlashControl.systemBacklash = backlashSize;

			// We just reached the end of the backlash which makes this value 0
			this->backlashControl.positionWithinBacklash = 0;
		}
		else {
			this->backlashControl.systemBacklash = 0;
			this->backlashControl.positionWithinBacklash = 0;
			log(LogLevel::Status, moduleName, "Negative backlash detected - presuming zero");
		}
		
		this->healthStatus.backlashOK = true;

		// Give a guess for homing if we haven't homed
		if(!this->healthStatus.homeOK) {
			// We are off the switch just at the disengage position
			// To get to the center of the switch we need to move through the backlash and then half way into the swtich
			// Backlash control will handle the backlash for us. So we just think about being half way off the switch center
			this->setCurrentPosition(- this->homing.switchSize / 2);
		}

		endRoutine();
		return Exception::None();
	}

	//----------
	Exception
	MotionControl::homeRoutine(const MeasureRoutineSettings& settings)
	{
		// Create a moduleName
		char moduleName[100];
		sprintf(moduleName, "%s.homeRoutine", this->getName());

		// Stop any existing motion profile
		this->stop();

		if(!this->timer.hardwareTimer) {
			return Exception(moduleName, "No hardware timer");
		}

		auto & homeSwitch = this->homeSwitch;
		auto microStepsPerStep = this->motorDriverSettings.getMicrostepsPerStep();
		const auto microstepsPerPrismRotation = this->getMicrostepsPerPrismRotation();

		HAL_Delay(10);

		// Start measuring time for timeout
		uint32_t startTime = millis();
		uint32_t timeoutTime = startTime + (uint32_t) settings.timeout_s * 1000U;

		log(LogLevel::Status, moduleName, "begin");

		this->switchesArmed = true;

		auto endRoutine = [this, &moduleName]() {
			this->stop();
			this->switchesArmed = false;
			log(LogLevel::Status, moduleName, "end");
		};

		const Steps buttonClearDistance = MOTION_CLEAR_SWITCH_STEPS * microStepsPerStep; // here we have a value by trial and error (at 128 microsteps)

		Steps positionForwardSwitchAccurate;
		log(LogLevel::Status, moduleName, "1: Find FW switch accurate");
		{
			auto result = this->routineFindSwitchAccurate(true
				, settings.slowMoveSpeed
				, true
				, timeoutTime);

			if(result.exception) {
				endRoutine();
				result.exception.setModuleName(moduleName);
				return result.exception;
			}

			positionForwardSwitchAccurate = result.frameSwitchEvents.forwards.positionSeen;
		}

		Steps positionBackwardsSwitchAccurate;
		log(LogLevel::Status, moduleName, "2: Find BW switch accurate");
		{
			auto result = this->routineFindSwitchAccurate(false
				, settings.slowMoveSpeed
				, false
				, timeoutTime);

			if(result.exception) {
				endRoutine();
				result.exception.setModuleName(moduleName);
				return result.exception;
			}

			positionBackwardsSwitchAccurate = result.frameSwitchEvents.backwards.positionSeen;
		}

		Steps homePosition;
		{

			// Now:
			// * We're moving backward
			// * We are outisde of backlash region (since we did actually move)
			// * We had backlash control on in the interrupt, so should be already backlash corrected
			homePosition = (positionForwardSwitchAccurate + positionBackwardsSwitchAccurate) / 2;
			this->homing.switchSize = positionBackwardsSwitchAccurate - positionForwardSwitchAccurate;
		}
			
		// Measure the current position at end of sequence
		{
			auto homePositionInDegrees = 360.0f * (float) homePosition / (float) microstepsPerPrismRotation;
			char message[100];
			sprintf(message
				, "Home = %d (%d/10 degrees )"
				, homePosition
				, (int) (homePositionInDegrees * 10));
			log(LogLevel::Status, moduleName, message);
		}

		{
			auto switchSizeInDegrees = 360.0f * (float) this->homing.switchSize / (float) microstepsPerPrismRotation;
			char message[100];
			sprintf(message
				, "Switch size = %d (%d/10 degrees )"
				, this->homing.switchSize
				, (int) (switchSizeInDegrees * 10));
			log(LogLevel::Status, moduleName, message);
		}
		
		this->position -= homePosition;
		this->targetPosition = 0;
		this->healthStatus.homeOK = true;
		this->healthStatus.switchesOK = true; // Since we used both forwards and backwards in this routine

		endRoutine();
		return Exception::None();
	}

	//----------
	Exception
	MotionControl::measureCycleRoutine(const MeasureRoutineSettings& settings)
	{
		// Create a moduleName
		char moduleName[100];
		sprintf(moduleName, "%s.measureCycleRoutine", this->getName());

		log(LogLevel::Status, moduleName, "begin");

		this->switchesArmed = true;

		auto endRoutine = [this, &moduleName]() {
			this->stop();
			this->switchesArmed = false;
			log(LogLevel::Status, moduleName, "end");
		};

		// Stop any existing motion profile
		this->stop();

		auto timeout_ms = settings.timeout_s * 1000U;
		auto clearSwitchDistance = MOTION_CLEAR_SWITCH_STEPS * this->motorDriverSettings.getMicrostepsPerStep();

		Steps positionAtStart;
		log(LogLevel::Status, moduleName, "1: Find the FW switch");
		{
			auto result = this->routineFindSwitchAccurate(true
				, settings.slowMoveSpeed
				, true
				, millis() + timeout_ms);

			if(result.exception.report()) {
				result.exception.setModuleName(moduleName);
				endRoutine();
				return result.exception;
			}

			positionAtStart = result.frameSwitchEvents.forwards.positionSeen;
		}

		log(LogLevel::Status, moduleName, "2: Move off the switch");
		{
			auto result = this->routineMoveTo(positionAtStart + clearSwitchDistance, millis() + timeout_ms);

			if(result.exception.report()) {
				result.exception.setModuleName(moduleName);
				endRoutine();
				return result.exception;
			}
		}

		Steps positionAtEnd;
		log(LogLevel::Status, moduleName, "3: Cycle to next FW switch");
		{
			auto result = this->routineFindSwitchAccurate(true
				, settings.slowMoveSpeed
				, false
				, millis() + timeout_ms);

			if(result.exception.report()) {
				result.exception.setModuleName(moduleName);
				endRoutine();
				return result.exception;
			}

			positionAtEnd = result.frameSwitchEvents.forwards.positionSeen;
		}

		// Calculate the length of one cycle
		auto cycleLength = positionAtEnd - positionAtStart;

		// Normalise to full steps
		cycleLength = cycleLength / this->motorDriverSettings.getMicrostepsPerStep();

		// Log the result
		{
			char message[100];
			sprintf(message, "Cycle = %d full steps (expected %d)", cycleLength, MOTION_STEPS_PER_PRISM_ROTATION);
			log(LogLevel::Status, moduleName, message);
		}

		// Check the result
		{
			auto delta = abs(cycleLength - MOTION_STEPS_PER_PRISM_ROTATION);
			if(delta > MOTION_ALLOWED_PRISM_ROTATION_ERROR) {
				char message[100];
				sprintf(message, "Cycle length error (%d > %d full steps)", delta, MOTION_ALLOWED_PRISM_ROTATION_ERROR);
				endRoutine();
				return Exception(moduleName, message);
			}
		}

		endRoutine();

		this->healthStatus.measureCycleOK = true;
		
		return Exception::None();
	}

	//----------
	void
	MotionControl::reportStatus(msgpack::Serializer& serializer)
	{
		serializer.beginMap(6);
		{
			serializer << "position" << this->position;
			serializer << "targetPosition" << this->targetPosition;

			serializer << "healthStatus";
			{
				serializer.beginMap(4);
				serializer << "measureCycleOK" << this->healthStatus.measureCycleOK;
				serializer << "SwitchesOK" << this->healthStatus.switchesOK;
				serializer << "backlashOK" << this->healthStatus.backlashOK;
				serializer << "homeOK" << this->healthStatus.homeOK;
			}

			serializer << "maximumSpeed" << this->motionProfile.maximumSpeed;
			serializer << "acceleration" << this->motionProfile.acceleration;
			serializer << "minimumSpeed" << this->motionProfile.minimumSpeed;
		}
	}

	//----------
	const MotionControl::HealthStatus &
	MotionControl::getHealthStatus() const
	{
		return this->healthStatus;
	}

	//----------
	MotionControl::RoutineMoveResult
	MotionControl::routineMoveTo(Steps targetPosition
		, uint32_t timeout)
	{
		// create moduleName
		char moduleName[100];
		sprintf(moduleName, "%s.routineMoveTo", this->getName());

		MotionControl::RoutineMoveResult result;

		this->setTargetPosition(targetPosition);

		while(this->position != this->targetPosition) {
			this->update();

			{
				char message[64];
				sprintf(message, "\r%d\t", this->position);
				Logger::X().printRaw(message);
			}

			// Check if timeout
			if (millis() > timeout) {
				result.exception = Exception::Timeout(moduleName);
				this->stop();
				return result;
			}

			// Check if should exit
			if(App::updateFromRoutine()) {
				result.exception = Exception::Escape(moduleName);
				this->stop();
				return result;
			}

			// Delay so can move
			HAL_Delay(1);
		}

		this->stop();

		return result;
	}

	//----------
	MotionControl::RoutineMoveResult
	MotionControl::routineMoveToUntilSeeSwitch(Steps targetPosition
			, SwitchesMask switchesMask
			, uint32_t timeout)
	{
		// create moduleName
		char moduleName[100];
		sprintf(moduleName, "%s.routineMoveToUntilSeeSwitch", this->getName());

		MotionControl::RoutineMoveResult result;

		this->setTargetPosition(targetPosition);

		while(true) {
			this->update();

			{
				char message[64];
				sprintf(message, "\r%d\t", this->position);
				Logger::X().printRaw(message);
			}

			// Check if switches seen
			auto frameSwitchEvents = this->getFrameSwitchEvents();
			if((switchesMask.forwards && frameSwitchEvents.forwards.seen)
				|| (switchesMask.backwards && frameSwitchEvents.backwards.seen)) {
				result.frameSwitchEvents = frameSwitchEvents;
				this->stop();
				return result;
			}

			// Check if we've got to target position (without seeing switch)
			if(this->getTargetPosition() == this->getPosition()) {
				result.exception = Exception::SwitchNotSeen(moduleName);
				this->stop();
				return result;
			}

			// Check if timeout
			if (millis() > timeout) {
				result.exception = Exception::Timeout(moduleName);
				this->stop();
				return result;
			}

			// Check if should exit
			if(App::updateFromRoutine()) {
				result.exception = Exception::Escape(moduleName);
				this->stop();
				return result;
			}

			// Delay so can move
			HAL_Delay(1);
		}
	}

	//----------
	MotionControl::RoutineMoveResult
	MotionControl::routineMoveToFindSwitch(bool direction
		, StepsPerSecond speed
		, MotionControl::SwitchesMask switchesMask
		, uint32_t timeout)
	{
		// create moduleName
		char moduleName[100];
		sprintf(moduleName, "%s.routineMoveToFindSwitch", this->getName());

		MotionControl::RoutineMoveResult result;

		this->run(direction, speed);

		while(true) {
			this->updateStepsAndSwitches();

			{
				char message[64];
				sprintf(message, "\r%d\t", this->position);
				Logger::X().printRaw(message);
			}

			// Check if switches seen
			auto frameSwitchEvents = this->getFrameSwitchEvents();
			if((switchesMask.forwards && frameSwitchEvents.forwards.seen)
				|| (switchesMask.backwards && frameSwitchEvents.backwards.seen)) {
				result.frameSwitchEvents = frameSwitchEvents;
				this->stop();
				return result;
			}

			// Check if timeout
			if (millis() > timeout) {
				result.exception = Exception::Timeout(moduleName);
				this->stop();
				return result;
			}

			// Check if should exit
			if(App::updateFromRoutine()) {
				result.exception = Exception::Escape(moduleName);
				this->stop();
				return result;
			}

			// Delay so can move
			HAL_Delay(1);
		}
		this->stop();
	}

	//----------
	MotionControl::RoutineMoveResult
	MotionControl::routineFindSwitchAccurate(bool direction
		, StepsPerSecond slowSpeed
		, bool guessPosition
		, uint32_t timeout)
	{
		// create moduleName
		char moduleName[100];
		sprintf(moduleName, "%s.routineFindSwitchAccurate", this->getName());

		auto forwardsVector = direction ? 1 : -1;
		auto backwardsVector = direction ? -1 : 1;
		auto switchesMask = direction ? SwitchesMask { true, false } : SwitchesMask { false, true };

		// Check if we are on the switch
		bool isOnSwitch = this->homeSwitch.getForwardsActive() || this->homeSwitch.getBackwardsActive();

		if(!isOnSwitch) {
			// Move onto the switch
			if(guessPosition) {
				const auto currentPosition = this->getPosition();

				// Calculate a target position based on the direction, also add an additional rotation for good measure
				auto targetPosition = this->getClosestHomePosition();
				if(targetPosition > currentPosition) {
					targetPosition += this->getMicrostepsPerPrismRotation();
				}
				else {
					targetPosition -= this->getMicrostepsPerPrismRotation();
				}

				// Go to closest cycle
				auto result = this->routineMoveToUntilSeeSwitch(targetPosition
					, SwitchesMask { true, true }
					, timeout);

				if(result.exception) {
					result.exception.setModuleName(moduleName);
					return result;
				}
			}
			else {
				// Move up to two full cycles forwards
				auto result = this->routineMoveToUntilSeeSwitch(this->position + forwardsVector * 2 * this->getMicrostepsPerPrismRotation()
					, switchesMask
					, timeout);
				
				if(result.exception) {
					result.exception.setModuleName(moduleName);
					return result;
				}
			}
		}

		// Now we're definitely on the switch (FW or BW - not known)

		// Move off the switch backwards
		{
			// Move off the switch
			auto result = this->routineMoveTo(this->position + backwardsVector * MOTION_CLEAR_SWITCH_STEPS * this->motorDriverSettings.getMicrostepsPerStep()
				, timeout);
			
			if(result.exception) {
				result.exception.setModuleName(moduleName);
				return result;
			}
		}

		// Now we're behind the switch

		// Move forwards onto the switch slowly
		Steps positionSwitchSeen;
		{
			auto result = this->routineMoveToFindSwitch(direction, slowSpeed, switchesMask, timeout);
			if(result.exception) {
				result.exception.setModuleName(moduleName);
				return result;
			}
			positionSwitchSeen = result.frameSwitchEvents.forwards.positionSeen;
		}

		// Return result
		MotionControl::RoutineMoveResult result;
		{
			if(direction) {
				result.frameSwitchEvents.forwards.seen = true;
				result.frameSwitchEvents.forwards.positionSeen = positionSwitchSeen;
			}
			else {
				result.frameSwitchEvents.backwards.seen = true;
				result.frameSwitchEvents.backwards.positionSeen = positionSwitchSeen;
			}
		}
		return result;
	}

#ifndef HOME_SWITCH_LEGACY
	// fastHomeRoutine -- self-calibrating optical home
	// =========================================================================================
	// Replaces BOTH measureBacklashRoutine + homeRoutine for the optical switch in
	// Routines::calibrate(): one pass produces the home centre, the switch size, and the
	// backlash. Ported from HomeSwitchTest/portalfw_port/FastHomeRoutine.cpp (bench-validated
	// 2026-07-10, 2,337/2,338-home overnight bake) with its threshold-calibration phase
	// replaced by the band-centred design in
	// HomeSwitchTest/reports/newring/HOME_ROUTINE_DESIGN.md, which is what's actually validated
	// against the injection-moulded ring in production -- the old "T = background - margin"
	// scheme is structurally inapplicable to it (the background never crosses at any threshold).
	//
	// Algorithm:
	//   0 GUARD      three settled background probes ~120deg apart; T_cap = (max measured
	//                crossing) - 3, or a hard dark-ring fallback if none of the three probes
	//                found a crossing at all. Cold runs only.
	//   1 SEEK       forward up to 1.25 rev at T_cap (cold) or the cached T_op (warm), with the
	//                (debounced) switch latch armed. Background is inactive by construction, so
	//                any span found is the flag.
	//   1c DETECT    (ratio unconfirmed) continue to the NEXT leading edge; lead-to-lead
	//                distance classifies the module as 32:1 or 16:1 and selects the matching
	//                FastHomeParams. Until confirmed the seek runs at the SLOWER generation's
	//                cruise speed -- the 16:1 motor stalls hard at the 32:1 cruise.
	//   2 BAND       (cold only) step T down from T_cap in 2-count steps; at each step a LOCAL
	//                two-edge re-scan (no full lap) measures the flag width. T_floor = lowest T
	//                still giving a plausible width; T_shoulder = lowest T where width blows out
	//                past the shoulder. Gate: band = (T_shoulder-1) - T_floor must be >= 6
	//                counts ("insufficient optical contrast" otherwise).
	//   3 OPERATE    (cold only) T_op = T_floor + round(0.55 * band); cache T_op and the width
	//                measured there (W_cal). Every width-relative gate below is scaled from
	//                W_cal, not an absolute constant -- this ring's flag is 130-190 microsteps,
	//                an order of magnitude narrower than the 32:1 params were originally tuned
	//                for.
	//   4 PRECISE    FASTHOME_PASSES forward two-edge passes (averaged), each latching the
	//                leading then trailing edge in the same engaged frame -- no backlash model
	//                needed for the midpoint. A width outside the W_cal-relative gate here (on a
	//                warm run in particular) is exactly the >25% drift that HOME_ROUTINE_DESIGN
	//                .md says should force recalibration -- fail() clears the cache so the next
	//                attempt runs cold.
	//   5 BACKLASH   walk past the trailing edge, verify disengaged, reverse, latch re-entry:
	//                backlash = trail - reenter.
	//   6 APPLY      position -= home; park via a forward-engaged approach (same frame as the
	//                edge pass).
	//
	// CALLER POLICY (Routines::calibrate): after a COLD run (threshold was not cached coming
	// in), run this routine ONCE MORE immediately -- the second run is warm and its datum is the
	// one to keep. A cold run's datum can sit up to ~114 microsteps (0.2 deg) off the warm one
	// (the calibration probing perturbs the thermo-optical profile right before the precise
	// pass); warm homes repeat to within a few microsteps.
	//
	// Deliberately simplified vs. the bench original (bench_main.cpp fastHome, command O): no
	// false-feature reject/re-seek loop, no flank-blip retry, no telemetry streaming -- relies
	// on the tryCount retry loop Routines::calibrate wraps this in instead. Lift those loops
	// across (they are straight copies in the bench source) if field units show frequent gate
	// failures.

	// ---- per-generation constants (bench-frozen; see PORTING.md for provenance) -----------
	struct FastHomeParams {
		uint8_t                 ratio;          // output gear ratio (set name), logging only
		StepsPerSecond          seekSpeed;      // forward seek cruise, >=20% below the stall cliff
		StepsPerSecondPerSecond seekAccel;
		StepsPerSecond          reposSpeed;     // datum-critical approach moves (slip here biases
		                                        // the datum, unlike the coarse seek)
		StepsPerSecond          edgeSpeed;      // precise pass; the datum depends on this value
		uint16_t                coarseDebounceM; // debounce for the coarse seek/detect phases,
		                                        // before the flag width (and so the calibrated
		                                        // debounce) is known
		Steps                   clearance;      // > backlash, room to arm
		Steps                   takeup;         // overshoot-return, forward-engaged
		Steps                   coarseWidthMax; // generation-scale estimate, used only to make
		                                        // sure the flag has been exited (Phase 1c) --
		                                        // NOT a precision gate
		Steps                   backlashMax;
		Steps                   ustepsPerRev;   // must match getMicrostepsPerPrismRotation()
	};

	// 32:1 (original modules). Rev = exact rational 32*118*9759*32/(296*21) rounded.
	static const FastHomeParams FASTHOME_32 =
		{ 32, 24000, 100000, 24000, 2000, 32, 5000, 2000, 4200, 3000, 189704 };
	// 16:1 (2026 modules). Rev = 92,252 MEASURED (nominal half-rational 94,852 is 2.8% wrong).
	// BACKWARD has a mid-band resonance that stalls the motor dead at a 5-6k cruise cold, so
	// all datum-critical approaches run at 4k, below any band.
	static const FastHomeParams FASTHOME_16 =
		{ 16, 14000, 100000, 4000, 2000, 48, 2500, 1000, 2100, 1500, 92252 };

	#define FASTHOME_T_CAP_DARK        253    // background guard fallback: no measurable background
	#define FASTHOME_BG_GUARD_MARGIN   3      // T_cap = bg - 3 when the background IS measurable
	#define FASTHOME_CAL_BRACKET_LO    140    // settled-probe bracket [lo..255]
	#define FASTHOME_CAL_ITERS         5      // binary probes (resolution ~4 duty)
	#define FASTHOME_CAL_SETTLE_MS     220    // per probe; threshold RC tau ~100 ms
	#define FASTHOME_SETTLE_MS        300     // after a large DAC step
	#define FASTHOME_BAND_STEP         2      // duty counts per band-scan step
	#define FASTHOME_BAND_MIN          6      // minimum acceptable [T_floor..T_shoulder) band
	#define FASTHOME_WIDTH_MIN_ABS     40     // W_MIN: minimum plausible flag width, microsteps
	#define FASTHOME_SHOULDER_FACTOR   3.0f   // width > this * W_med => shoulder onset
	#define FASTHOME_T_OP_FRACTION     0.55f  // T_op = T_floor + round(F * band)
	#define FASTHOME_MAX_BAND_STEPS    32     // safety cap on the Phase 2 scan (~6 expected)
	#define FASTHOME_DEBOUNCE_MIN      8
	#define FASTHOME_DEBOUNCE_MAX      32
	#define FASTHOME_PASSES            2      // bench-measured sweet spot; more passes let
	                                          // thermal drift outpace averaging

	// ---- helper: settled crossing probe at the current position ---------------------------
	// Binary-search the comparator flip over [lo..255] with a real RC settle per probe (the
	// threshold DAC is RC-filtered, tau ~100 ms -- fast sweeps read ~20+ duty high and MUST NOT
	// be used to derive thresholds). Returns the crossing duty; -1 = stuck (railLo tells which
	// rail); -2 = aborted. Leaves the DAC at the last probe -- caller restores it.
	static int fastHomeSettledCrossingProbe(HomeSwitch& homeSwitch, bool& railLo, uint32_t timeoutTime)
	{
		auto settle = [&](uint8_t duty) -> bool {
			HomeSwitch::setThreshold(duty);
			uint32_t until = millis() + FASTHOME_CAL_SETTLE_MS;
			while(millis() < until) {
				if(App::updateFromRoutine()) return false;
				if(millis() > timeoutTime) return false;
				HAL_Delay(1);
			}
			return true;
		};

		railLo = false;
		if(!settle(FASTHOME_CAL_BRACKET_LO)) return -2;
		const bool atLo = homeSwitch.getForwardsActive();
		if(!settle(255)) return -2;
		const bool atHi = homeSwitch.getForwardsActive();
		if(atLo == atHi) {
			railLo = atLo;
			return -1;
		}
		int lo = FASTHOME_CAL_BRACKET_LO, hi = 255;
		for(int i = 0; i < FASTHOME_CAL_ITERS; i++) {
			int mid = (lo + hi) / 2;
			if(!settle((uint8_t) mid)) return -2;
			if(homeSwitch.getForwardsActive() == atLo) lo = mid; else hi = mid;
		}
		return (lo + hi) / 2;
	}

	//----------
	Exception
	MotionControl::fastHomeRoutine(const MeasureRoutineSettings& settings)
	{
		const char * moduleName = this->getName();
		this->stop();
		if(!this->timer.hardwareTimer) {
			return Exception(moduleName, "No hardware timer");
		}

		const auto microstepsPerStep = this->motorDriverSettings.getMicrostepsPerStep();
		if(!this->fastHomeParams) {
			this->fastHomeParams = &FASTHOME_32; // default guess until Phase 1c confirms
		}
		const FastHomeParams * p = this->fastHomeParams;
		Steps lapBudget = p->ustepsPerRev + p->ustepsPerRev / 4;
		const uint32_t timeoutTime = millis() + (uint32_t) settings.timeout_s * 1000U;
		const MotionProfile normalProfile = this->getMotionProfile();

		auto endRoutine = [this]() {
			this->stop();
			this->switchesArmed = false;
			this->inInterrupt.invertSwitches = false;
		};
		// Every failure exit goes through here: clears the calibration cache (so the next
		// attempt recalibrates cold), restores the default threshold and the caller's motion
		// profile, and stops. Folding the profile restore in here (rather than repeating it
		// before every `return fail(...)`) is deliberate -- that repetition is exactly the kind
		// of thing that's easy to miss at one call site among a dozen.
		auto fail = [&](const char * msg) -> Exception {
			this->opticalThresholdCached = 0;
			this->opticalWidthCached = 0;
			HomeSwitch::setThreshold(HOMESWITCHOPTICAL_DEFAULT_THRESHOLD);
			this->setMotionProfile(normalProfile);
			endRoutine();
			return Exception(moduleName, msg);
		};

		StepsPerSecond seekSpeed = p->seekSpeed;
		if(!this->fastHomeRatioConfirmed) {
			if(FASTHOME_16.seekSpeed < seekSpeed) seekSpeed = FASTHOME_16.seekSpeed;
			if(FASTHOME_32.seekSpeed < seekSpeed) seekSpeed = FASTHOME_32.seekSpeed;
		}
		MotionProfile seekProfile;
		seekProfile.maximumSpeed = seekSpeed;
		seekProfile.acceleration = p->seekAccel;
		seekProfile.minimumSpeed = 1000;
		this->setMotionProfile(seekProfile);
		this->switchLatchDebounce = p->coarseDebounceM;

		const bool warm = (this->opticalThresholdCached > 0) && (this->opticalWidthCached > 0);
		int T = warm ? this->opticalThresholdCached : 0;
		Steps W_cal = warm ? this->opticalWidthCached : 0;

		// ---- Phase 0: background guard (cold runs only) ------------------------------------
		int T_cap = T; // warm: seek directly at the cached operating threshold
		if(!warm) {
			int maxBg = -1;
			bool anyMeasured = false;
			for(int i = 0; i < 3; i++) {
				if(i > 0) {
					auto r = this->routineMoveTo(this->getPosition() + p->ustepsPerRev / 3, timeoutTime);
					if(r.exception) return fail("abort");
				}
				bool rail;
				int c = fastHomeSettledCrossingProbe(this->homeSwitch, rail, timeoutTime);
				if(c == -2) return fail("abort");
				if(c >= 0) {
					anyMeasured = true;
					if(c > maxBg) maxBg = c;
				}
			}
			T_cap = anyMeasured ? (maxBg - FASTHOME_BG_GUARD_MARGIN) : FASTHOME_T_CAP_DARK;
			if(T_cap < 16) T_cap = 16;
			if(T_cap > 255) T_cap = 255;
		}
		HomeSwitch::setThreshold((uint8_t) T_cap);
		HAL_Delay(FASTHOME_SETTLE_MS);

		// ---- Phase 1: seek a coarse leading edge --------------------------------------------
		this->switchesArmed = true;
		if(this->homeSwitch.getForwardsActive()) {
			// Background should be inactive by construction at T_cap (cold) or T_op (warm; T_op
			// is always <= the T_cap that would have been derived from the same background, see
			// Phase 3). If it's active at rest here the world has moved since the threshold was
			// set -- fail and let the caller's retry recalibrate cold, rather than the bench
			// original's adaptive T-walk (see the simplification note above).
			return fail("active at rest");
		}
		Steps coarseLead;
		{
			auto r = this->routineMoveToUntilSeeSwitch(this->getPosition() + lapBudget,
				SwitchesMask{ true, false }, timeoutTime);
			if(r.exception || !r.frameSwitchEvents.forwards.seen) {
				return fail("flag not found");
			}
			coarseLead = r.frameSwitchEvents.forwards.positionSeen;
		}

		// ---- Phase 1c: motor generation detection (lead-to-lead = one revolution) -----------
		if(!this->fastHomeRatioConfirmed) {
			if(this->homeSwitch.getForwardsActive()) {
				this->inInterrupt.invertSwitches = true;
				this->updateStepsAndSwitches();
				auto r = this->routineMoveToUntilSeeSwitch(
					this->getPosition() + p->coarseWidthMax + p->takeup,
					SwitchesMask{ true, false }, timeoutTime);
				this->inInterrupt.invertSwitches = false;
				if(r.exception || !r.frameSwitchEvents.forwards.seen) {
					return fail("motor detect: stuck active");
				}
			}
			auto r = this->routineMoveTo(this->getPosition() + p->takeup, timeoutTime);
			if(r.exception) return fail(r.exception.getMessage().c_str());
			r = this->routineMoveToUntilSeeSwitch(this->getPosition() + lapBudget,
				SwitchesMask{ true, false }, timeoutTime);
			if(r.exception || !r.frameSwitchEvents.forwards.seen) {
				return fail("motor detect: flag not found");
			}
			const Steps lead2 = r.frameSwitchEvents.forwards.positionSeen;
			const Steps measuredRev = lead2 - coarseLead;
			const FastHomeParams * detected = nullptr;
			for(const FastHomeParams * cand : { &FASTHOME_16, &FASTHOME_32 }) {
				const Steps expect = cand->ustepsPerRev;
				if(measuredRev > expect - expect / 10 && measuredRev < expect + expect / 10) {
					detected = cand;
				}
			}
			if(!detected) return fail("motor detect: rev out of range");
			this->fastHomeParams = detected;
			this->fastHomeRatioConfirmed = true;
			p = detected;
			lapBudget = p->ustepsPerRev + p->ustepsPerRev / 4;
			this->switchLatchDebounce = p->coarseDebounceM;
			seekProfile.maximumSpeed = p->seekSpeed;
			seekProfile.acceleration = p->seekAccel;
			this->setMotionProfile(seekProfile);
			coarseLead = lead2;
		}

		// ---- Phase 2/3: band-centred threshold calibration (cold runs only) -----------------
		// See HomeSwitchTest/reports/newring/HOME_ROUTINE_DESIGN.md. Replaces the old
		// background-margin scheme, which has no inputs on a ring whose background never
		// crosses at any threshold.
		if(!warm) {
			// Local width probe: jog to leadGuess - clearance, then latch lead+trail forward at
			// edgeSpeed within a short, bounded local search. Returns width, or -1 if not found.
			auto localWidthProbe = [&](Steps leadGuess) -> Steps {
				auto r = this->routineMoveTo(leadGuess - p->clearance, timeoutTime);
				if(r.exception) return -1;
				uint32_t localDeadline = millis() + 3000;
				if(localDeadline > timeoutTime) localDeadline = timeoutTime;
				r = this->routineMoveToFindSwitch(true, p->edgeSpeed, SwitchesMask{ true, false }, localDeadline);
				if(r.exception || !r.frameSwitchEvents.forwards.seen) return -1;
				Steps lead = r.frameSwitchEvents.forwards.positionSeen;

				this->inInterrupt.invertSwitches = true;
				this->updateStepsAndSwitches();
				localDeadline = millis() + 3000;
				if(localDeadline > timeoutTime) localDeadline = timeoutTime;
				r = this->routineMoveToFindSwitch(true, p->edgeSpeed, SwitchesMask{ true, false }, localDeadline);
				this->inInterrupt.invertSwitches = false;
				if(r.exception || !r.frameSwitchEvents.forwards.seen) return -1;
				Steps trail = r.frameSwitchEvents.forwards.positionSeen;

				return trail - lead;
			};

			struct BandSample { int T; Steps width; };
			BandSample samples[FASTHOME_MAX_BAND_STEPS];
			int sampleCount = 0;
			int belowFloorStreak = 0;
			int Tscan = T_cap;
			while(sampleCount < FASTHOME_MAX_BAND_STEPS && Tscan >= 16) {
				if(millis() > timeoutTime) return fail("timeout");
				if(App::updateFromRoutine()) return fail("abort");
				HomeSwitch::setThreshold((uint8_t) Tscan);
				HAL_Delay(FASTHOME_CAL_SETTLE_MS);
				Steps width = localWidthProbe(coarseLead);
				if(width >= 0) {
					samples[sampleCount++] = BandSample{ Tscan, width };
					belowFloorStreak = (width < FASTHOME_WIDTH_MIN_ABS) ? belowFloorStreak + 1 : 0;
				}
				else {
					belowFloorStreak++;
				}
				if(belowFloorStreak >= 2 && sampleCount > 0) break;
				Tscan -= FASTHOME_BAND_STEP;
			}
			if(sampleCount == 0) return fail("band scan: flag not found at any threshold");

			// Median width across found samples (insertion sort -- sampleCount is tiny).
			Steps widths[FASTHOME_MAX_BAND_STEPS];
			for(int i = 0; i < sampleCount; i++) widths[i] = samples[i].width;
			for(int i = 1; i < sampleCount; i++) {
				Steps v = widths[i];
				int j = i - 1;
				while(j >= 0 && widths[j] > v) { widths[j + 1] = widths[j]; j--; }
				widths[j + 1] = v;
			}
			Steps W_med = widths[sampleCount / 2];
			if(sampleCount % 2 == 0 && sampleCount > 1) {
				W_med = (widths[sampleCount / 2 - 1] + widths[sampleCount / 2]) / 2;
			}
			if(W_med <= 0) return fail("band scan: degenerate width");

			int T_floor = 256, T_shoulder = -1;
			for(int i = 0; i < sampleCount; i++) {
				if(samples[i].width >= FASTHOME_WIDTH_MIN_ABS && samples[i].T < T_floor) {
					T_floor = samples[i].T;
				}
				if((float) samples[i].width > FASTHOME_SHOULDER_FACTOR * (float) W_med
					&& (T_shoulder < 0 || samples[i].T < T_shoulder)) {
					T_shoulder = samples[i].T;
				}
			}
			if(T_floor > 255) return fail("band scan: no width above the floor");
			// No shoulder observed within the scanned range: Phase 0 already put T_cap below
			// the measured background, so the true shoulder is at or above T_cap by construction.
			if(T_shoulder < 0) T_shoulder = T_cap + 1;

			int band = (T_shoulder - 1) - T_floor;
			if(band < FASTHOME_BAND_MIN) return fail("insufficient optical contrast");

			int T_op = T_floor + (int) round(FASTHOME_T_OP_FRACTION * (float) band);
			if(T_op < T_floor + 2) T_op = T_floor + 2;
			if(T_op > T_shoulder - 2) T_op = T_shoulder - 2;

			HomeSwitch::setThreshold((uint8_t) T_op);
			HAL_Delay(FASTHOME_SETTLE_MS);
			Steps W_atOp = localWidthProbe(coarseLead);
			if(W_atOp < 0) return fail("operating point: flag not found");

			T = T_op;
			W_cal = W_atOp;
		}

		HomeSwitch::setThreshold((uint8_t) T);
		HAL_Delay(FASTHOME_SETTLE_MS);
		if(this->homeSwitch.getForwardsActive()) {
			return fail("shoulder gate: active below lead");
		}

		// Width-relative gates and debounce, scaled from the calibrated flag width so they
		// auto-scale to whatever ring is actually attached (HOME_ROUTINE_DESIGN.md).
		Steps widthMin = (Steps)((float) W_cal * 0.65f);
		Steps widthMax = (Steps)((float) W_cal * 1.35f);
		uint16_t calibratedDebounce = (uint16_t)(W_cal / 8);
		if(calibratedDebounce < FASTHOME_DEBOUNCE_MIN) calibratedDebounce = FASTHOME_DEBOUNCE_MIN;
		if(calibratedDebounce > FASTHOME_DEBOUNCE_MAX) calibratedDebounce = FASTHOME_DEBOUNCE_MAX;
		this->switchLatchDebounce = calibratedDebounce;

		// ---- Phase 4: precise forward two-edge pass(es) -------------------------------------
		MotionProfile reposProfile = seekProfile;
		reposProfile.maximumSpeed = p->reposSpeed;
		{
			this->setMotionProfile(reposProfile);
			auto r = this->routineMoveTo(coarseLead - p->clearance - p->takeup, timeoutTime);
			if(!r.exception) r = this->routineMoveTo(coarseLead - p->clearance, timeoutTime);
			this->setMotionProfile(seekProfile);
			if(r.exception) return fail(r.exception.getMessage().c_str());
		}
		if(this->homeSwitch.getForwardsActive()) {
			return fail("active at pass arming point");
		}

		Steps lead = 0, trail = 0;
		Steps midSum = 0, widthSum = 0;
		for(int pass = 0; pass < FASTHOME_PASSES; pass++) {
			if(pass > 0) {
				this->setMotionProfile(reposProfile);
				auto r = this->routineMoveTo(coarseLead - p->clearance - p->takeup, timeoutTime);
				if(!r.exception) r = this->routineMoveTo(coarseLead - p->clearance, timeoutTime);
				this->setMotionProfile(seekProfile);
				if(r.exception) return fail(r.exception.getMessage().c_str());
				if(this->homeSwitch.getForwardsActive()) {
					return fail("active at pass arming point");
				}
			}

			auto r = this->routineMoveToFindSwitch(true, p->edgeSpeed, SwitchesMask{ true, false }, timeoutTime);
			if(r.exception || !r.frameSwitchEvents.forwards.seen) {
				return fail("leading edge");
			}
			lead = r.frameSwitchEvents.forwards.positionSeen;

			this->inInterrupt.invertSwitches = true;
			this->updateStepsAndSwitches();
			r = this->routineMoveToFindSwitch(true, p->edgeSpeed, SwitchesMask{ true, false }, timeoutTime);
			this->inInterrupt.invertSwitches = false;
			if(r.exception || !r.frameSwitchEvents.forwards.seen) {
				return fail("trailing edge");
			}
			trail = r.frameSwitchEvents.forwards.positionSeen;

			if(trail - lead < widthMin || trail - lead > widthMax) {
				// On a warm run this is exactly the >25% width-drift-from-W_cal recalibration
				// trigger in HOME_ROUTINE_DESIGN.md -- fail() clears the cache, so the next
				// attempt (the tryCount retry Routines::calibrate wraps this in) runs cold.
				return fail("width gate");
			}
			midSum += (lead + trail) / 2;
			widthSum += trail - lead;
		}
		const Steps width = (Steps)(widthSum / FASTHOME_PASSES);
		const Steps home = (Steps)(midSum / FASTHOME_PASSES);

		// ---- Phase 5: backlash at the trailing edge ------------------------------------------
		this->backlashControl.systemBacklash = 0;
		this->backlashControl.positionWithinBacklash = 0;
		{
			auto r = this->routineMoveTo(trail + settings.debounceDistance * microstepsPerStep, timeoutTime);
			if(r.exception) return fail(r.exception.getMessage().c_str());
		}
		if(this->homeSwitch.getForwardsActive()) {
			return fail("no disengage after trailing edge");
		}
		Steps reenter;
		{
			auto r = this->routineMoveToFindSwitch(false, p->edgeSpeed, SwitchesMask{ false, true }, timeoutTime);
			if(r.exception || !r.frameSwitchEvents.backwards.seen) {
				return fail("backlash re-entry");
			}
			reenter = r.frameSwitchEvents.backwards.positionSeen;
		}
		Steps backlash = trail - reenter;
		if(backlash < -32 || backlash > p->backlashMax) {
			return fail("backlash out of range");
		}
		if(backlash < 0) backlash = 0;

		// ---- Phase 6: apply --------------------------------------------------------------------
		{
			this->setMotionProfile(reposProfile);
			auto r = this->routineMoveTo(home - p->clearance - p->takeup, timeoutTime);
			if(!r.exception) r = this->routineMoveTo(home - p->clearance, timeoutTime);
			if(!r.exception) r = this->routineMoveTo(home, timeoutTime);
			this->setMotionProfile(seekProfile);
			if(r.exception) return fail(r.exception.getMessage().c_str());
		}

		this->backlashControl.systemBacklash = backlash;
		this->backlashControl.positionWithinBacklash = 0;
		this->homing.switchSize = width;
		this->position -= home;
		this->targetPosition = 0;
		this->opticalThresholdCached = (int16_t) T;
		this->opticalWidthCached = W_cal;
		this->healthStatus.homeOK = true;
		this->healthStatus.backlashOK = true;
		this->healthStatus.switchesOK = true;

		this->setMotionProfile(normalProfile);
		endRoutine();
		return Exception::None();
	}
#endif
}