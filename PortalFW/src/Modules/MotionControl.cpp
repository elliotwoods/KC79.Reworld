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
				// One read for both latches. getForwardsActive() and getBackwardsActive() are
				// the same pin on the optical switch, so asking twice bought nothing and cost
				// the expensive half of this ISR twice over. See HomeSwitchOptical::getRawState.
				const auto raw = this->homeSwitch.getRawState();

				if(!switchesSeen.forwards.seen) {
					if (raw.forwards ^ this->inInterrupt.invertSwitches) {
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
					if (raw.backwards ^ this->inInterrupt.invertSwitches) {
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
			bool succeeded = false;
			for(uint8_t i=0; i<settings.tryCount; i++) {
				auto exception = this->unjamRoutine(settings);
				if(exception) {
					log(exception);
				}
				else {
					succeeded = true;
					break;
				}
			}
			// Unlike the pre-2026 sweep, unjamRoutine ends with the datum zeroed and homeOK
			// false whenever it got as far as moving (see unjamEnd) -- deliberately NOT chased
			// with an automatic home here, so the caller stays in control of when the module
			// moves again. Say so loudly: a host that treats unjam as datum-preserving (as the
			// old firmware was) would otherwise carry on in an arbitrary position frame.
			if(this->getUnjamReport().ran) {
				log(LogLevel::Warning, this->getName()
					, "unjam: home datum LOST (position zeroed, homeOK=false); send home before absolute moves");
			}
			return succeeded;
		}
#ifdef HOME_SWITCH_LEGACY
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
#endif
		else if(strcmp(key, "home") == 0) {
			// HOMING ROUTINE
			//
			// An optical board homes with fastHomeRoutine, which produces the datum, the flag
			// width and the backlash in one pass. This key used to run the legacy mechanical
			// homeRoutine on every board including v6, so the only homing reachable over the
			// wire was the wrong one for the hardware it was talking to.
			MeasureRoutineSettings settings;
			if(!MotionControl::readMeasureRoutineSettings(stream, settings)) {
				return false;
			}
			for(uint8_t i=0; i<settings.tryCount; i++) {
#ifdef HOME_SWITCH_LEGACY
				auto exception = this->homeRoutine(settings);
#else
				auto exception = this->fastHomeRoutine(settings);
#endif
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

		// Pull step count from interrupt.
		//
		// Read-then-zero is two instructions, and a step landing between them had its increment
		// thrown away -- one microstep of position lost, permanently, every time it happened.
		// Rare per frame; not rare over a show, and it accumulates in one direction. The
		// critical section is those two instructions and nothing else.
		Steps stepCount;
		{
			const uint32_t primask = __get_PRIMASK();
			__disable_irq();
			stepCount = this->inInterrupt.stepCount;
			this->inInterrupt.stepCount = 0;
			__set_PRIMASK(primask);
		}
		
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
	// The unjam sweep, in three parts so Routines::unjam can step both axes at once.
	//
	// Full torque and full steps are the CALLER's to set, because both come from the one
	// MotorDriverSettings the two axes share -- see the header.
	Exception
	MotionControl::unjamBegin(const MeasureRoutineSettings&
		, const UnjamSettings& unjamSettings)
	{
		auto& state = this->unjamState;
		state = UnjamState();
		state.settings = unjamSettings;

		char moduleName[100];
		sprintf(moduleName, "%s.unjam", this->getName());

		if(!this->timer.hardwareTimer) {
			return Exception(moduleName, "No hardware timer");
		}

		// Stop any existing motion profile before taking the operating point over
		this->stop();

		// One prism revolution, in the unit this sweep runs in. The caller has already put the
		// driver in full-step mode, so this reads back as full steps per revolution.
		const auto revolution = this->getMicrostepsPerPrismRotation();

		state.stride = unjamSettings.stride > 0
			? unjamSettings.stride
			: revolution / 4;
		if(state.stride < 8) {
			return Exception(moduleName, "Stride too small");
		}

		state.travelTarget = revolution * (Steps) unjamSettings.rotations;
		state.report.expectedGap = revolution;

		// Priors this axis owns. The shared driver settings are the caller's.
		state.priorMotionProfile = this->motionProfile;
		state.priorSystemBacklash = this->backlashControl.systemBacklash;
		state.priorPositionWithinBacklash = this->backlashControl.positionWithinBacklash;
		state.priorLatchDebounce = this->switchLatchDebounce;
		state.priorSwitchesArmed = this->switchesArmed;

		// Whatever we knew about where home is, we are about to drive through it on purpose
		this->healthStatus.homeOK = false;

		// The backlash model is expressed in microsteps and this sweep counts full steps, so
		// leaving it engaged would inject a quarter-revolution dead band at every one of the
		// dozens of reversals below, and corrupt the very distances the sweep exists to
		// measure. Zeroed for the duration; homing re-measures it afterwards anyway.
		this->backlashControl.systemBacklash = 0;
		this->backlashControl.positionWithinBacklash = 0;

		// An RS485 move that arrived just before the routine leaves the filter armed, and
		// updateFilteredMotion would then rewrite targetPosition under us on every tick.
		this->motionFiltering.active = false;

		// Watch the flag on the way past. Two consecutive agreeing samples, not one: in
		// full-step mode the flag is only about nine samples wide, so a long debounce would
		// swallow it, while a one-shot latch lets comparator noise on the dip flank through as
		// a phantom sighting.
		this->switchesArmed = true;
		this->switchLatchDebounce = 2;
		this->updateStepsAndSwitches(); // clear any stale events out

		{
			MotionProfile unjamMotionProfile;
			unjamMotionProfile.acceleration = unjamSettings.acceleration;
			unjamMotionProfile.maximumSpeed = unjamSettings.speed;
			unjamMotionProfile.minimumSpeed = 100;
			this->setMotionProfile(unjamMotionProfile);
		}

		state.origin = this->getPosition();
		state.nextRevolutionMark = revolution;
		state.forwards = true;
		state.offFlag = !this->homeSwitch.getForwardsActive();
		state.deadline = millis() + (uint32_t) unjamSettings.timeout_s * 1000U;
		state.active = true;
		state.report.ran = true;

		this->setTargetPosition(state.origin + state.stride);

		{
			char message[110];
			sprintf(message, "begin: %d fwd / %d back full steps, clearing %d rev = %d steps, at %dmA"
				, (int) state.stride
				, (int) (state.stride / 2)
				, (int) unjamSettings.rotations
				, (int) state.travelTarget
				, (int) (this->motorDriverSettings.getCurrent() * 1000.0f));
			log(LogLevel::Status, moduleName, message);
		}

		return Exception::None();
	}

	//----------
	bool
	MotionControl::unjamUpdate()
	{
		auto& state = this->unjamState;
		if(!state.active) {
			return false;
		}

		this->update();

		const auto position = this->getPosition();

		// A sighting is the FIRST forward crossing of the flag since we were last off it. Only
		// forward legs count, so every sighting is the same edge approached the same way and
		// the distances between them are comparable.
		if(state.forwards
			&& this->frameSwitchEvents.forwards.seen
			&& state.offFlag) {
			const auto seenAt = this->frameSwitchEvents.forwards.positionSeen;
			state.offFlag = false;

			auto& report = state.report;

			// The same flag, again. Every cycle steps back half a stride, so a flag crossed
			// near the end of one forward leg is usually crossed again at the start of the
			// next -- with the default stride that is roughly every other cycle. Those are one
			// sighting, not several: the next DIFFERENT flag is a whole revolution away, and
			// slip only ever makes that distance longer, never shorter, so half a revolution
			// separates the two cases with room to spare.
			const bool sameFlagAgain = state.haveSighting
				&& seenAt - state.lastSighting < report.expectedGap / 2;

			if(!sameFlagAgain) {
				char moduleName[100];
				sprintf(moduleName, "%s.unjam", this->getName());

				if(!state.haveSighting) {
					state.haveSighting = true;
					report.firstSighting = seenAt;
					report.sightings = 1;

					char message[80];
					sprintf(message, "home 1 at %+d (first)", (int) (seenAt - state.origin));
					log(LogLevel::Status, moduleName, message, false);
				}
				else {
					const Steps gap = seenAt - state.lastSighting;
					const Steps deviation = gap - report.expectedGap;
					const Steps absDeviation = deviation < 0 ? -deviation : deviation;

					if(report.sightings == 1 || gap < report.shortestGap) {
						report.shortestGap = gap;
					}
					if(report.sightings == 1 || gap > report.longestGap) {
						report.longestGap = gap;
					}
					if(absDeviation > report.worstDeviation) {
						report.worstDeviation = absDeviation;
					}
					report.totalAbsDeviation += absDeviation;
					if(report.sightings < 255) {
						report.sightings++;
					}

					char message[100];
					sprintf(message, "home %d at %+d: gap %d, %+d vs one revolution"
						, (int) report.sightings
						, (int) (seenAt - state.origin)
						, (int) gap
						, (int) deviation);
					log(LogLevel::Status, moduleName, message, false);
				}
				report.lastSighting = seenAt;
				state.lastSighting = seenAt;
			}
		}

		if(!this->homeSwitch.getForwardsActive()) {
			state.offFlag = true;
		}

		if(millis() > state.deadline) {
			state.timedOut = true;
			state.active = false;
			return false;
		}

		if(state.forwards) {
			if(position < this->getTargetPosition()) {
				return true;
			}

			const Steps travelled = position - state.origin;
			state.report.netTravel = travelled;

			// Progress a revolution at a time rather than a cycle at a time: with the default
			// stride that is ten lines instead of eighty, and a revolution is the unit the
			// operator is actually waiting on.
			while(travelled >= state.nextRevolutionMark) {
				char moduleName[100];
				sprintf(moduleName, "%s.unjam", this->getName());
				char message[100];
				sprintf(message, "cleared %d/%d rev (%d cycles, %d home sightings)"
					, (int) (state.nextRevolutionMark / state.report.expectedGap)
					, (int) state.settings.rotations
					, (int) state.report.cycles
					, (int) state.report.sightings);
				log(LogLevel::Status, moduleName, message, false);
				state.nextRevolutionMark += state.report.expectedGap;
			}

			if(travelled >= state.travelTarget) {
				state.report.completed = true;
				state.active = false;
				return false;
			}

			state.forwards = false;
			this->setTargetPosition(position - state.stride / 2);
			return true;
		}

		if(position > this->getTargetPosition()) {
			return true;
		}

		state.report.cycles++;
		state.forwards = true;
		this->setTargetPosition(position + state.stride);
		return true;
	}

	//----------
	Exception
	MotionControl::unjamEnd()
	{
		auto& state = this->unjamState;

		char moduleName[100];
		sprintf(moduleName, "%s.unjam", this->getName());

		this->stop();

		if(!state.report.ran) {
			return Exception::None();
		}

		state.report.netTravel = this->getPosition() - state.origin;
		const bool stoppedEarly = state.active;
		state.active = false;

		// Restore this axis's priors. The shared driver settings are the caller's to put back.
		this->setMotionProfile(state.priorMotionProfile);
		this->backlashControl.systemBacklash = state.priorSystemBacklash;
		this->backlashControl.positionWithinBacklash = state.priorPositionWithinBacklash;
		this->switchLatchDebounce = state.priorLatchDebounce;
		this->switchesArmed = state.priorSwitchesArmed;

		// The datum is gone, and pretending otherwise would be worse than saying so: `position`
		// has been counting FULL steps into the variable the rest of the firmware reads as
		// microsteps, so the only honest value to leave behind is zero. Homing re-datums, and
		// healthStatus.homeOK has been false since unjamBegin.
		this->zeroCurrentPosition();

		auto& report = state.report;
		{
			char message[110];
			sprintf(message, "%s: %d cycles, %d full steps net, %d home sightings"
				, report.completed ? "cleared" : (state.timedOut ? "TIMED OUT" : "stopped")
				, (int) report.cycles
				, (int) report.netTravel
				, (int) report.sightings);
			log(report.completed ? LogLevel::Status : LogLevel::Warning, moduleName, message);
		}

		if(report.sightings >= 2) {
			char message[120];
			sprintf(message, "home consistency: gap %d..%d against %d expected, worst %d, mean |dev| %d"
				, (int) report.shortestGap
				, (int) report.longestGap
				, (int) report.expectedGap
				, (int) report.worstDeviation
				, (int) (report.totalAbsDeviation / (int32_t) (report.sightings - 1)));
			log(LogLevel::Status, moduleName, message);
		}

		log(LogLevel::Status, moduleName, "end");

		// Verdicts, worst first. Timing out and being stopped both mean "we do not know"; only
		// a sweep that ran its whole distance can say anything about the flag.
		if(state.timedOut) {
			return Exception::Timeout(moduleName);
		}
		if(stoppedEarly) {
			return Exception::Escape(moduleName);
		}

		// Seeing the flag once per revolution is the whole confirmation that the prism turned
		// with the motor -- position counts steps COMMANDED, so it reaches the target whether
		// anything moved or not, and the flag is the only witness that it did. One sighting
		// short is allowed for the lap the sweep started part-way into.
		const int expectedSightings = (int) state.settings.rotations;
		if(report.sightings == 0) {
			return Exception(moduleName, "Home flag never seen");
		}
		if((int) report.sightings + 1 < expectedSightings) {
			char message[80];
			sprintf(message, "Home flag seen %d times in %d revolutions"
				, (int) report.sightings
				, expectedSightings);
			return Exception(moduleName, message);
		}

		// A prism turning cleanly repeats its revolution to well inside 1%. More scatter than
		// that is the motor losing steps against a load, which is what a jam looks like from
		// here, and is the answer this routine exists to give.
		if(report.worstDeviation > report.expectedGap / 100) {
			char message[90];
			sprintf(message, "Home position drifts up to %d full steps per revolution"
				, (int) report.worstDeviation);
			return Exception(moduleName, message);
		}

		return Exception::None();
	}

	//----------
	const MotionControl::UnjamReport &
	MotionControl::getUnjamReport() const
	{
		return this->unjamState.report;
	}

	//----------
	Exception
	MotionControl::unjamRoutine(const MeasureRoutineSettings& settings)
	{
		return this->unjamRoutine(settings, UnjamSettings());
	}

	//----------
	Exception
	MotionControl::unjamRoutine(const MeasureRoutineSettings& settings
		, const UnjamSettings& unjamSettings)
	{
#ifdef UNJAM_DISABLED
		return Exception::None();
#else
		char moduleName[100];
		sprintf(moduleName, "%s.unjamRoutine", this->getName());

		log(LogLevel::Status, moduleName, "begin");

		// One axis, so this function owns the shared operating point (see unjamBegin).
		const auto priorCurrent = this->motorDriverSettings.getCurrent();
		const auto priorMicrostep = this->motorDriverSettings.getMicrostepResolution();
		this->motorDriverSettings.setCurrent(MOTORDRIVERSETTINGS_MAX_CURRENT);
		this->motorDriverSettings.setMicrostepResolution(MotorDriverSettings::MicrostepResolution::_1);

		auto restore = [&]() {
			this->motorDriverSettings.setMicrostepResolution(priorMicrostep);
			this->motorDriverSettings.setCurrent(priorCurrent);
			log(LogLevel::Status, moduleName, "end");
		};

		auto exception = this->unjamBegin(settings, unjamSettings);
		if(exception) {
			this->unjamEnd();
			restore();
			return exception;
		}

		bool escaped = false;
		while(this->unjamUpdate()) {
			HAL_Delay(2);
			if(App::updateFromRoutine()) {
				escaped = true;
				break;
			}
		}

		auto result = this->unjamEnd();
		restore();

		if(escaped) {
			return Exception::Escape(moduleName);
		}
		return result;
#endif
	}

	//----------
	// The cycle check, in three parts so Routines::cycleCheck can step both axes at once.
	//
	// See the header for what it is asking and why the answer is worth having early.
	Exception
	MotionControl::cycleCheckBegin(const CycleCheckSettings& settings)
	{
		auto& state = this->cycleCheckState;
		state = CycleCheckState();
		state.settings = settings;

		char moduleName[100];
		sprintf(moduleName, "%s.cycleCheck", this->getName());

		if(!this->timer.hardwareTimer) {
			return Exception(moduleName, "No hardware timer");
		}

		this->stop();

		state.revolution = this->getMicrostepsPerPrismRotation();
		state.toleranceUsteps = settings.tolerance * this->motorDriverSettings.getMicrostepsPerStep();

		// How close two sightings have to be before they are the same flag rather than two.
		//
		// The latch re-arms every frame, so while we are ON the flag it keeps reporting an
		// entry; and a comparator that dithers on the dip flank can drop out and re-enter
		// inside the flag. This absorbs both.
		//
		// Expressed as a fraction of a revolution rather than as a microstep count, because a
		// microstep count is only true at one microstep resolution: the flag measures ~266
		// microsteps at 32 (FASTHOME_W_DEFAULT), and 8 at full steps. A 128th of a revolution
		// is about five and a half flag widths at any resolution. It is also nowhere near a
		// real second feature -- the one this module has sits at 0.57 of a revolution, seventy
		// times further out.
		state.sameFlagWindow = state.revolution / 128;

		state.priorMotionProfile = this->motionProfile;
		state.priorSystemBacklash = this->backlashControl.systemBacklash;
		state.priorPositionWithinBacklash = this->backlashControl.positionWithinBacklash;
		state.priorLatchDebounce = this->switchLatchDebounce;
		state.priorSwitchesArmed = this->switchesArmed;

		// This is the flag's only witness, so say so before we have looked: if the check does
		// not reach a verdict, the module must not still be claiming the cycle is good.
		this->healthStatus.measureCycleOK = false;

		// Backlash compensation off for the duration, so `position` is exactly the count of
		// steps commanded and the distance between two sightings is exactly what the motor did.
		// It also matters at the very first move: an axis whose last motion was backwards would
		// otherwise spend systemBacklash microsteps not advancing `position` at all, and a flag
		// crossed inside that window would be recorded in the wrong place.
		this->backlashControl.systemBacklash = 0;
		this->backlashControl.positionWithinBacklash = 0;

		// An RS485 move that arrived just before startup leaves the filter armed, and
		// updateFilteredMotion would rewrite targetPosition under us on every tick.
		this->motionFiltering.active = false;

		this->switchesArmed = true;

		// Consecutive agreeing samples before the latch believes an edge. A quarter of a full
		// step matches the optical routine's coarse floor (FASTHOME_DEBOUNCE_MIN, 8) at the
		// production 32 microsteps, and stays sane at coarser resolutions where the flag is
		// only a handful of samples wide -- a fixed 8 would swallow it whole at full steps.
		{
			const Steps microstepsPerStep = this->motorDriverSettings.getMicrostepsPerStep();
			const Steps debounce = microstepsPerStep / 4;
			this->switchLatchDebounce = (uint16_t) (debounce < 2 ? 2 : debounce);
		}

		this->updateStepsAndSwitches(); // drop any stale event

		{
			MotionProfile profile;
			profile.maximumSpeed = settings.speed;
			this->setMotionProfile(profile);
		}

		state.origin = this->getPosition();
		state.offFlag = !this->homeSwitch.getForwardsActive();
		state.deadline = millis() + (uint32_t) settings.timeout_s * 1000U;
		state.verdict = CycleCheckVerdict::Working;
		state.active = true;

		// A revolution and a bit to find the flag at all -- the bit covers starting just past
		// it, and the ramp up to speed.
		this->setTargetPosition(state.origin + state.revolution * 115 / 100);

		return Exception::None();
	}

	//----------
	bool
	MotionControl::cycleCheckUpdate()
	{
		auto& state = this->cycleCheckState;
		if(!state.active) {
			return false;
		}

		this->update();
		const auto position = this->getPosition();

		// A sighting is a fresh forward entry onto the flag: latched by the ISR, and only once
		// the sensor has read inactive since the last one.
		bool sighting = false;
		Steps seenAt = 0;
		if(this->frameSwitchEvents.forwards.seen && state.offFlag) {
			seenAt = this->frameSwitchEvents.forwards.positionSeen;
			state.offFlag = false;
			sighting = true;
		}
		if(!this->homeSwitch.getForwardsActive()) {
			state.offFlag = true;
		}

		if(state.parking) {
			if(position > this->getTargetPosition()) {
				return true;
			}
			state.active = false;
			return false;
		}

		if(sighting) {
			char moduleName[100];
			sprintf(moduleName, "%s.cycleCheck", this->getName());

			if(!state.confirming) {
				state.confirming = true;
				state.firstSighting = seenAt;

				// From here we care about one revolution, measured from the flag rather than
				// from wherever the axis happened to be parked.
				this->setTargetPosition(seenAt + state.revolution + state.toleranceUsteps);

				char message[80];
				sprintf(message, "flag seen after %d usteps; timing one revolution"
					, (int) (seenAt - state.origin));
				log(LogLevel::Status, moduleName, message, false);
			}
			else {
				const Steps gap = seenAt - state.firstSighting;

				if(gap < state.sameFlagWindow) {
					// Still the same flag. Not a second feature, and not worth a log line.
				}
				else if(gap < state.revolution - state.toleranceUsteps) {
					// A second feature, arriving early. This is the fast fail, and it is the
					// fault this module has: the flag the homing routine locks onto is not the
					// only thing on the ring that crosses the threshold.
					state.spacing = gap;
					state.verdict = CycleCheckVerdict::ExtraFlag;
					state.active = false;

					char message[100];
					sprintf(message, "second flag after %d usteps, %d%% of a revolution"
						, (int) gap
						, (int) ((int64_t) gap * 100 / (int64_t) state.revolution));
					log(LogLevel::Error, moduleName, message);
					return false;
				}
				else {
					state.spacing = gap;
					state.verdict = CycleCheckVerdict::Passed;

					char message[100];
					sprintf(message, "one revolution = %d usteps (%+d against %d)"
						, (int) gap
						, (int) (gap - state.revolution)
						, (int) state.revolution);
					log(LogLevel::Status, moduleName, message);

					// Park with the flag a short way ahead, so the homing routine that follows
					// acquires it almost immediately instead of seeking a whole revolution for
					// something we have just been looking straight at.
					state.parking = true;
					this->setTargetPosition(seenAt
						- MOTION_CLEAR_SWITCH_STEPS * this->motorDriverSettings.getMicrostepsPerStep());
					return true;
				}
			}
		}

		if(millis() > state.deadline) {
			state.verdict = CycleCheckVerdict::Timeout;
			state.active = false;
			return false;
		}

		if(position >= this->getTargetPosition()) {
			state.verdict = state.confirming
				? CycleCheckVerdict::WrongSpacing
				: CycleCheckVerdict::NoFlag;
			state.active = false;
			return false;
		}

		return true;
	}

	//----------
	Exception
	MotionControl::cycleCheckEnd()
	{
		auto& state = this->cycleCheckState;

		char moduleName[100];
		sprintf(moduleName, "%s.cycleCheck", this->getName());

		this->stop();

		if(state.verdict == CycleCheckVerdict::NotRun) {
			return Exception::None();
		}

		// Stopped with the state machine still mid-flight: the operator escaped, or the caller
		// gave up on us. Either way we did not reach a verdict, and must not imply one.
		if(state.active || state.verdict == CycleCheckVerdict::Working) {
			state.verdict = CycleCheckVerdict::Aborted;
		}
		state.active = false;

		this->setMotionProfile(state.priorMotionProfile);
		this->backlashControl.systemBacklash = state.priorSystemBacklash;
		this->backlashControl.positionWithinBacklash = state.priorPositionWithinBacklash;
		this->switchLatchDebounce = state.priorLatchDebounce;
		this->switchesArmed = state.priorSwitchesArmed;

		if(state.verdict == CycleCheckVerdict::Passed) {
			this->healthStatus.measureCycleOK = true;
			log(LogLevel::Status, moduleName, "pass");
			return Exception::None();
		}

		char message[100];
		switch(state.verdict) {
		case CycleCheckVerdict::NoFlag:
			sprintf(message, "no flag in a revolution -- jammed, or the sensor cannot see it");
			break;
		case CycleCheckVerdict::ExtraFlag:
			sprintf(message, "flag seen twice per revolution (second at %d of %d usteps)"
				, (int) state.spacing, (int) state.revolution);
			break;
		case CycleCheckVerdict::WrongSpacing:
			sprintf(message, "flag did not return within a revolution +/- %d usteps"
				, (int) state.toleranceUsteps);
			break;
		case CycleCheckVerdict::Timeout:
			sprintf(message, "timed out after %ds", (int) state.settings.timeout_s);
			break;
		default:
			sprintf(message, "stopped before a verdict");
			break;
		}
		return Exception(moduleName, message);
	}

	//----------
	MotionControl::CycleCheckVerdict
	MotionControl::getCycleCheckVerdict() const
	{
		return this->cycleCheckState.verdict;
	}

	//----------
	Steps
	MotionControl::getCycleCheckSpacing() const
	{
		return this->cycleCheckState.spacing;
	}

	//----------
	const char *
	MotionControl::cycleCheckVerdictName(CycleCheckVerdict verdict)
	{
		switch(verdict) {
		case CycleCheckVerdict::Passed:       return "pass";
		case CycleCheckVerdict::NoFlag:       return "no flag";
		case CycleCheckVerdict::ExtraFlag:    return "extra flag";
		case CycleCheckVerdict::WrongSpacing: return "wrong spacing";
		case CycleCheckVerdict::Timeout:      return "timeout";
		case CycleCheckVerdict::Aborted:      return "aborted";
		case CycleCheckVerdict::Working:      return "working";
		default:                              return "not run";
		}
	}

#ifdef HOME_SWITCH_LEGACY
	// Mechanical (PCB v4) only. On the optical build fastHomeRoutine produces the datum, the
	// flag width and the backlash in one pass, so this is dead weight in an application image
	// that is already 98% full.

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
#endif


	//----------
	void
	MotionControl::reportStatus(msgpack::Serializer& serializer)
	{
#ifndef HOME_SWITCH_LEGACY
		serializer.beginMap(9);
#else
		serializer.beginMap(6);
#endif
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
#ifndef HOME_SWITCH_LEGACY
			serializer << "opticalThreshold" << this->opticalThresholdCached;
			serializer << "opticalWidth" << this->opticalWidthCached;
			serializer << "fastHomeFailure" << (uint8_t) this->lastFastHomeFailure;
#endif
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
	//                any span found is the flag. On a cold run the seek does not stop at the
	//                leading edge: it carries straight on to latch the TRAILING edge too, so the
	//                flag's coarse extent at T_cap comes out of the pass that was happening
	//                anyway (see "acquisition hand-off" below).
	//   2 CROSSING   (cold only) walk a few points across the acquired span, measuring the
	//                flag's own settled crossing duty and keeping the brightest. With the
	//                background censored the usable window is simply (crossing .. T_cap], so no
	//                width-vs-threshold contrast is needed.
	//   3 OPERATE    (cold only) T_op = crossing + round(0.55 * usable); cache T_op and the width
	//                measured there (W_cal). Every width-relative gate below is scaled from
	//                W_cal, not an absolute constant -- this ring's flag is 130-190 microsteps
	//                unpainted, an order of magnitude narrower than the 32:1 params were
	//                originally tuned for, and several times wider again once painted.
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
	//
	// GEARING: 32:1 only. There was a lead-to-lead detection phase here that classified the
	// module as 32:1 or 16:1 by measuring one revolution, and it cost a full extra revolution on
	// the first cold home of every power-up plus a permanent clamp of the seek to the 16:1
	// cruise speed (14,000 instead of 24,000 microsteps/s) until it had run. 32:1 is the
	// production gearing -- it sustains >=60 deg/s of prism against the 16:1's thermally-limited
	// 23-27 -- so the detection was paying that price on every unit to keep a path open for a
	// generation that is not built any more. The 16:1 constants are recorded in
	// HomeSwitchTest/reports (usteps/rev 92,252 measured, seek 14,000, repos 4,000, debounce 48)
	// if a 16:1 unit ever has to be serviced.

	// ---- geometry / motion constants (bench-frozen; see PORTING.md for provenance) ----------
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
		Steps                   coarseWidthMax; // scale estimate for how far the flag can smear
		                                        // at the acquisition threshold -- used to bound
		                                        // local searches, NOT a precision gate
		Steps                   backlashMax;
		Steps                   ustepsPerRev;   // must match getMicrostepsPerPrismRotation()
	};

	// 32:1, the production gearing. Rev = exact rational 32*118*9759*32/(296*21) rounded.
	static const FastHomeParams FASTHOME_32 =
		{ 32, 24000, 100000, 24000, 2000, 32, 5000, 2000, 4200, 3000, 189704 };

	// Starting acquisition ceiling when the background is censored -- i.e. when the ring never
	// crosses at any threshold, the measured situation on both axes of this board. It is only a
	// starting point: the acquisition loop in phase 1 walks it up or down per axis, because the
	// two axes on this board need thresholds about eleven counts apart and no single constant
	// serves both. Raising this to 255 outright was tried and is wrong -- at 255 axis A reads
	// active across more than a quarter revolution, so the ring does dither near the ceiling
	// even though three static background probes had all come back censored.
	#define FASTHOME_T_CAP_DARK        253
	#define FASTHOME_BG_GUARD_MARGIN   3      // T_cap = bg - 3 when the background IS measurable
	#define FASTHOME_CAL_BRACKET_LO    140    // settled-probe bracket [lo..255]
	#define FASTHOME_CAL_ITERS         5      // binary probes (resolution ~4 duty)
	#define FASTHOME_CAL_SETTLE_MS     220    // per probe; threshold RC tau ~100 ms
	#define FASTHOME_SETTLE_MS        300     // after a large DAC step
	// Minimum number of threshold counts between the flag's own crossing and the safe ceiling.
	// This is the signal-quality gate: below it there is nowhere to put an operating point that
	// is clear of both the flag and the dither at the top of the range. Deliberately small,
	// because the measured margin differs sharply between the two axes on this board (about 19
	// counts on A, about 2 on B) and refusing B outright is worse than running it close to the
	// ceiling with the width gates still watching.
	#define FASTHOME_MARGIN_MIN        2
	// How many times phase 1 may step the acquisition threshold before giving up. The two axes
	// here sit about eleven counts apart, so a handful of steps has to be able to cross that.
	#define FASTHOME_MAX_T_ADJUST      8
	#define FASTHOME_T_OP_FRACTION     0.55f  // T_op = crossing + round(F * usable)
	// How many points across the acquired span are probed for the flag's crossing. The span at
	// the acquisition threshold is a broad faint skirt and the bright core is not necessarily
	// central in it, so more than one point is needed; but after the first full probe each
	// further point is answered by a SINGLE settle (see fastHomeCrossingIsBrighterThan), so the
	// marginal cost of a point is ~220 ms rather than ~1.5 s.
	#define FASTHOME_CROSSING_POINTS   5
	// Creep speed for the T_cap span scan. Faster than edgeSpeed because that scan only spaces
	// out probe points -- no datum depends on it. W_cal still uses edgeSpeed.
	#define FASTHOME_CAL_SCAN_SPEED    8000

	// ---- the default operating point ---------------------------------------------------------
	// Measured by census (`:n`) on a production v6 module with SILVER-PAINTED home flags,
	// 2026-08-19, one lap per threshold from 175 to 255:
	//
	//        T    175  180  185  190  200  205  210  215  220  225  230  235  240  245  250  255
	//   A width    -    -    -    -    -    -    48   53   53  185  185  275  357  432  973 1896
	//   B width   83   52   83   53   38   41   53   49  193   53  260  258  300  330  866 1671
	//
	// Every lap that saw the flag saw exactly ONE segment -- no dither split anywhere, including
	// at 255, which is what paint buys. Axis A's floor is between 205 and 210; axis B was still
	// detecting at the bottom of the sweep. So the band both axes share is 210..255, 46 counts
	// wide, against 9-11 counts unpainted and 2 counts unpainted-under-a-cover.
	//
	// 235 is that band's operating point under the same rule the self-calibration uses
	// (floor + 0.55 * span = 210 + 25). It sits 25 counts above the worse axis's floor and 20
	// below the ceiling, and it is comfortably below the 250+ region where the flag smears past
	// 800 microsteps and the surround starts contributing.
	//
	// This is a DEFAULT, not an assumption: fastHomeRoutine verifies it against the flag it
	// actually finds and falls back to the full self-calibration if the verification fails, so a
	// module with a dim or missing flag still homes -- just slowly, and it says so in the log.
	#define FASTHOME_T_DEFAULT         235
	// Flag width at T_DEFAULT, microsteps. A measured 275 and B 258 in the census above.
	#define FASTHOME_W_DEFAULT         266
	// Width gate for the first pass on a SEEDED run, as a fraction of W_DEFAULT. Much looser
	// than the warm gate below, because W_DEFAULT is a fleet constant rather than this axis's
	// own measurement -- the census spread across two axes was already 258 to 275, and paint
	// thickness and ambient will widen that. Tight enough to reject a phantom or a smear; loose
	// enough not to reject a perfectly good flag for being 30% off the fleet average. Once a
	// pass succeeds the axis caches its OWN width and the normal 0.65/1.35 gate applies.
	#define FASTHOME_SEED_WIDTH_LO     0.40f
	#define FASTHOME_SEED_WIDTH_HI     2.50f
	// How many extra clearances the precise pass may step back looking for clear background to
	// arm from. Three covers a coarse-seek latch that overshot by four clearances; beyond that
	// the sensor is seeing something a flag-sized feature cannot explain and failing is right.
	#define FASTHOME_ARM_BACKOFFS      3
	#define FASTHOME_DEBOUNCE_MIN      8
	#define FASTHOME_DEBOUNCE_MAX      32
	#define FASTHOME_PASSES            2      // bench-measured sweet spot; more passes let
	#define FASTHOME_REPEAT_WIDTH_PCT  25     // seeded passes must agree within 25% of larger
	#define FASTHOME_REPEAT_MID_MAX    16     // and their datum midpoints within 16 microsteps
	#define FASTHOME_BACKLASH_NEG_TOL  512    // directional optical hysteresis; clamp to zero
	                                          // thermal drift outpace averaging

	void MotionControl::restoreOpticalCalibration(uint8_t threshold, Steps width) {
		// Flash values are only a starting point: fastHomeRoutine still verifies them with two
		// complete lead/trail passes. Reject implausible but CRC-valid values here so settings
		// corruption can never weaken the runtime gates.
		if(threshold >= 16 && width >= FASTHOME_DEBOUNCE_MIN
			&& width <= FASTHOME_32.coarseWidthMax) {
			this->opticalThresholdCached = threshold;
			this->opticalWidthCached = width;
		}
	}

	// ---- helper: set the shared threshold DAC and wait for it to actually be there -----------
	// The DAC is a software PWM through a 100k/1uF filter, tau ~100 ms. A reading taken before
	// it has settled is worth nothing -- swept measurements read 20+ duty counts high, which is
	// exactly how a "measured" threshold ends up outside the band it was supposed to be inside.
	// Returns false if the operator escaped or the routine deadline passed.
	static bool fastHomeSettle(uint8_t duty, uint32_t timeoutTime)
	{
		HomeSwitch::setThreshold(duty);
		uint32_t until = millis() + FASTHOME_CAL_SETTLE_MS;
		while(millis() < until) {
			if(App::updateFromRoutine()) return false;
			if(millis() > timeoutTime) return false;
			HAL_Delay(1);
		}
		return true;
	}

	// ---- helper: is the surface in front of the sensor brighter than `duty`? -----------------
	// One settle and one read: the cheapest question that can be asked of this sensor (~220 ms
	// against ~1.5 s for a full crossing probe). Returns 1 = yes (active, so crossing < duty),
	// 0 = no, -2 = aborted. Leaves the DAC at `duty`.
	//
	// This exists because the crossing scan does not need each point's crossing value -- it only
	// needs to know which point is BRIGHTEST. Once one point has been measured properly, every
	// later point can be dismissed or promoted by a single question against the best so far, and
	// only a point that actually beats it has to pay for a full search.
	static int fastHomeCrossingIsBrighterThan(HomeSwitch& homeSwitch, int duty, uint32_t timeoutTime)
	{
		if(duty < 0) return 0;
		if(duty > 255) duty = 255;
		if(!fastHomeSettle((uint8_t) duty, timeoutTime)) return -2;
		return homeSwitch.getForwardsActive() ? 1 : 0;
	}

	// ---- helper: settled crossing probe at the current position ---------------------------
	// Binary-search the comparator flip over [lo..hi] with a real RC settle per probe. Returns
	// the crossing duty; -1 = stuck (railLo tells which rail); -2 = aborted. Leaves the DAC at
	// the last probe -- caller restores it.
	//
	// `hi` is a caller-supplied ceiling and `knownActiveAtHi` says the caller has already
	// established the sensor reads active there. Both exist to skip work the caller has already
	// paid for: during calibration the flag is known to be active at the acquisition threshold,
	// so re-testing 255 is a wasted settle, and searching above T_cap is searching a region the
	// operating point can never be placed in anyway.
	static int fastHomeSettledCrossingProbeBounded(HomeSwitch& homeSwitch, bool& railLo
		, int hi, bool knownActiveAtHi, uint32_t timeoutTime)
	{
		if(hi > 255) hi = 255;
		if(hi <= FASTHOME_CAL_BRACKET_LO) hi = FASTHOME_CAL_BRACKET_LO + 1;

		railLo = false;
		if(!fastHomeSettle(FASTHOME_CAL_BRACKET_LO, timeoutTime)) return -2;
		const bool atLo = homeSwitch.getForwardsActive();
		bool atHi;
		if(knownActiveAtHi) {
			atHi = true;
		} else {
			if(!fastHomeSettle((uint8_t) hi, timeoutTime)) return -2;
			atHi = homeSwitch.getForwardsActive();
		}
		if(atLo == atHi) {
			railLo = atLo;
			return -1;
		}
		int lo = FASTHOME_CAL_BRACKET_LO;
		for(int i = 0; i < FASTHOME_CAL_ITERS; i++) {
			int mid = (lo + hi) / 2;
			if(!fastHomeSettle((uint8_t) mid, timeoutTime)) return -2;
			if(homeSwitch.getForwardsActive() == atLo) lo = mid; else hi = mid;
		}
		return (lo + hi) / 2;
	}

	// Unbounded form, for callers with no prior knowledge (the 'd' diagnostic, the background
	// guard). Searches the whole bracket and tests both rails.
	static int fastHomeSettledCrossingProbe(HomeSwitch& homeSwitch, bool& railLo, uint32_t timeoutTime)
	{
		return fastHomeSettledCrossingProbeBounded(homeSwitch, railLo, 255, false, timeoutTime);
	}

	//----------
	bool
	MotionControl::getHomeSwitchActive() const
	{
		return this->homeSwitch.getForwardsActive();
	}

	//----------
	int
	MotionControl::probeHomeCrossing(bool & railLo, uint32_t timeoutTime)
	{
		return fastHomeSettledCrossingProbe(this->homeSwitch, railLo, timeoutTime);
	}

	//----------
	// Safety cap on how many transitions one lap may report. A clean flag gives two. A threshold
	// up in the dither zone has given twenty-plus on this hardware, and the point of the census
	// is precisely to make that visible rather than to survive it, so the cap only exists to
	// bound the log and the run time.
	#define CENSUS_MAX_EDGES 40

	Exception
	MotionControl::homeSwitchCensusRoutine(uint8_t threshold
		, StepsPerSecond speed
		, const MeasureRoutineSettings& settings)
	{
		const char * moduleName = this->getName();
		this->stop();
		if(!this->timer.hardwareTimer) {
			return Exception(moduleName, "No hardware timer");
		}

		const FastHomeParams * const p = &FASTHOME_32;
		const uint32_t timeoutTime = millis() + (uint32_t) settings.timeout_s * 1000U;
		const MotionProfile normalProfile = this->getMotionProfile();
		const auto currentBefore = this->motorDriverSettings.getCurrent();
		const uint8_t thresholdBefore = HomeSwitch::getThreshold();
		const uint16_t debounceBefore = this->switchLatchDebounce;
		const Steps backlashBefore = this->backlashControl.systemBacklash;

		// Same current as homing, for the same reason: a lap that silently loses microsteps
		// reports its edges at the wrong positions, and a census whose positions are wrong is
		// worse than no census at all.
		this->motorDriverSettings.setCurrent(MOTORDRIVERSETTINGS_MAX_CURRENT);

		auto restore = [&]() {
			this->stop();
			this->switchesArmed = false;
			this->inInterrupt.invertSwitches = false;
			this->switchLatchDebounce = debounceBefore;
			this->setMotionProfile(normalProfile);
			this->motorDriverSettings.setCurrent(currentBefore);
			HomeSwitch::setThreshold(thresholdBefore);
			// Put the backlash model back -- the header promises the census does not touch it.
			// Every census move is forward, so on every exit path the mesh is forward-engaged
			// and the honest pending value is 0, not whatever was pending before the lap.
			this->backlashControl.systemBacklash = backlashBefore;
			this->backlashControl.positionWithinBacklash = 0;
		};

		if(speed == 0) speed = p->seekSpeed;
		MotionProfile censusProfile;
		censusProfile.maximumSpeed = speed;
		censusProfile.acceleration = p->seekAccel;
		censusProfile.minimumSpeed = 50; // see the note in fastHomeRoutine: routineMoveTo exits
		                                 // on exact equality and a high floor orbits the target
		this->setMotionProfile(censusProfile);

		// Raw commanded-position space, exactly as homing runs, so reported edge positions mean
		// the same thing they do in a homing log.
		this->backlashControl.systemBacklash = 0;
		this->backlashControl.positionWithinBacklash = 0;
		this->switchLatchDebounce = p->coarseDebounceM;

		if(!fastHomeSettle(threshold, timeoutTime)) {
			restore();
			return Exception::Escape(moduleName);
		}

		const Steps start = this->getPosition();
		const Steps end = start + p->ustepsPerRev;
		bool active = this->homeSwitch.getForwardsActive();
		this->switchesArmed = true;

		{
			char message[100];
			sprintf(message, "census begin: T=%d from=%d rev=%d speed=%d startActive=%d"
				, (int) threshold, (int) start, (int) p->ustepsPerRev, (int) speed
				, active ? 1 : 0);
			log(LogLevel::Status, moduleName, message);
		}

		int edges = 0;
		int segments = 0;
		Steps widest = 0;
		Steps widestAt = 0;
		Steps pendingLead = 0;
		bool havePendingLead = false;
		bool truncated = false;

		while(true) {
			if(edges >= CENSUS_MAX_EDGES) { truncated = true; break; }

			// Latch the transition we are NOT currently in: active now means watch for it to go
			// inactive, and vice versa.
			this->inInterrupt.invertSwitches = active;
			this->updateStepsAndSwitches();

			auto r = this->routineMoveToUntilSeeSwitch(end, SwitchesMask{ true, false }, timeoutTime);
			this->inInterrupt.invertSwitches = false;

			if(App::getShouldEscapeFromRoutine()) {
				restore();
				return Exception::Escape(moduleName);
			}
			if(millis() > timeoutTime) {
				restore();
				return Exception::Timeout(moduleName);
			}
			// Reaching the far end without another transition is the ordinary way a lap ends.
			if(r.exception || !r.frameSwitchEvents.forwards.seen) break;

			const Steps at = r.frameSwitchEvents.forwards.positionSeen;
			active = !active;
			edges++;

			if(active) {
				pendingLead = at;
				havePendingLead = true;
			} else if(havePendingLead) {
				const Steps width = at - pendingLead;
				segments++;
				havePendingLead = false;
				if(width > widest) { widest = width; widestAt = pendingLead; }
			}

			char message[100];
			sprintf(message, "  edge %d @%d -> %s\r\n", edges, (int) at
				, active ? "ACTIVE" : "inactive");
			Logger::X().printRaw(message);
		}

		// A segment still open at the end of the lap wraps past the start; report it rather than
		// dropping it, because "one wide segment straddling the lap boundary" and "no segment at
		// all" are very different answers.
		if(havePendingLead) {
			segments++;
			const Steps width = end - pendingLead;
			if(width > widest) { widest = width; widestAt = pendingLead; }
		}

		{
			char message[110];
			sprintf(message, "census end: T=%d edges=%d segs=%d widest=%d@%d%s"
				, (int) threshold, edges, segments, (int) widest, (int) widestAt
				, truncated ? " TRUNCATED" : "");
			log(LogLevel::Status, moduleName, message);
		}

		restore();
		return Exception::None();
	}

	//----------
	Exception
	MotionControl::fastHomeRoutine(const MeasureRoutineSettings& settings)
	{
		const char * moduleName = this->getName();
		this->lastFastHomeFailure = FastHomeFailure::None;
		this->stop();
		if(!this->timer.hardwareTimer) {
			return Exception(moduleName, "No hardware timer");
		}

		const auto microstepsPerStep = this->motorDriverSettings.getMicrostepsPerStep();
		const FastHomeParams * const p = &FASTHOME_32;
		const Steps lapBudget = p->ustepsPerRev + p->ustepsPerRev / 4;
		const uint32_t timeoutTime = millis() + (uint32_t) settings.timeout_s * 1000U;
		const MotionProfile normalProfile = this->getMotionProfile();

		// Normal homing uses the persisted module current. Routines owns the one explicit
		// full-current recovery retry, because both axes share this setting and a successful
		// promotion must be persisted module-wide rather than hidden inside one axis routine.
		auto endRoutine = [this]() {
			this->stop();
			this->switchesArmed = false;
			this->inInterrupt.invertSwitches = false;
		};
		// Every failure exit goes through here: clears the calibration cache (so the next
		// attempt recalibrates cold), marks the axis unhealthy, restores the default threshold
		// and the caller's motion profile, and stops. Folding all of that in here (rather than
		// repeating it before every `return fail(...)`) is deliberate -- that repetition is
		// exactly the kind of thing that's easy to miss at one call site among two dozen.
		//
		// Clearing healthStatus is what makes the LED fault pattern fire. LEDs::update() reads
		// these flags every loop tick and shows the 1-blink-A / 2-blink-B pattern whenever any
		// of them is false. Without this, an axis that homed successfully once and then started
		// failing would keep showing the healthy heartbeat forever -- the failure would be
		// invisible on the board itself, which is exactly the case the fault pattern exists for.
		// Declared before fail() so a seeded failure can disqualify the default; assigned once
		// the run's mode has been decided, below.
		bool seededRun = false;
		// Not every failure is evidence about the threshold, and treating them alike is
		// expensive. A run that cannot find the flag, arms onto it, or measures a width nothing
		// like the expected one is telling us the operating point is wrong for this axis --
		// recalibrating is the right answer. A backlash reading out of range, or a move that
		// returned an exception, says nothing whatsoever about the threshold: the flag was found
		// and resolved at it twice. Sending that case round a 60-second self-calibration cannot
		// help, and it is exactly what turned a 79 s startup into a 149 s one on a module whose
		// only problem was one odd backlash sample.
		//
		// So `fail()` implicates the threshold by default -- the conservative choice -- and
		// `failKeepingDefault()` is used where the cause is demonstrably something else. Both
		// still clear the cache and the health flags, so the retry re-measures either way.
		auto failureName = [](FastHomeFailure failure) -> const char * {
			switch(failure) {
				case FastHomeFailure::Aborted: return "aborted";
				case FastHomeFailure::Timeout: return "timeout";
				case FastHomeFailure::Motion: return "motion";
				case FastHomeFailure::FeatureMissing: return "feature-missing";
				case FastHomeFailure::FeatureTooWide: return "feature-too-wide";
				case FastHomeFailure::SpeedDependentEdge: return "speed-dependent-edge";
				case FastHomeFailure::SensorUnstable: return "sensor-unstable";
				case FastHomeFailure::OpticalContrast: return "optical-contrast";
				case FastHomeFailure::Backlash: return "backlash";
				default: return "none";
			}
		};
		auto failCommon = [&](const char * msg, bool implicatesThreshold
			, FastHomeFailure failure) -> Exception {
			this->lastFastHomeFailure = failure;
			{
				char diagnostic[120];
				sprintf(diagnostic, "fastHome result: status=fail class=%s reason=%s"
					, failureName(failure), msg);
				log(LogLevel::Error, moduleName, diagnostic);
			}
			// A run that started from the fleet default and could not finish has just produced
			// the one piece of evidence that matters about this axis: the default does not work
			// on it. Record that, so Routines' retry loop spends its next attempt measuring
			// rather than making the same failed assumption a second and third time.
			if(implicatesThreshold && seededRun && !this->opticalDefaultRejected) {
				this->opticalDefaultRejected = true;
				char note[90];
				sprintf(note, "SLOW PATH: default T=%d rejected (%s); recalibrating"
					, FASTHOME_T_DEFAULT, msg);
				log(LogLevel::Warning, moduleName, note);
			}
			if(implicatesThreshold) {
				this->opticalThresholdCached = 0;
				this->opticalWidthCached = 0;
			}
			this->healthStatus.homeOK = false;
			this->healthStatus.backlashOK = false;
			this->healthStatus.switchesOK = false;
			HomeSwitch::setThreshold(HOMESWITCHOPTICAL_DEFAULT_THRESHOLD);
			this->setMotionProfile(normalProfile);
			endRoutine();
			return Exception(moduleName, msg);
		};
		auto fail = [&](const char * msg) -> Exception {
			return failCommon(msg, true, FastHomeFailure::FeatureMissing);
		};
		auto failSpeed = [&](const char * msg) -> Exception {
			return failCommon(msg, true, FastHomeFailure::SpeedDependentEdge);
		};
		auto failUnstable = [&](const char * msg) -> Exception {
			return failCommon(msg, true, FastHomeFailure::SensorUnstable);
		};
		auto failContrast = [&](const char * msg) -> Exception {
			return failCommon(msg, true, FastHomeFailure::OpticalContrast);
		};
		auto failMotion = [&](const char * msg) -> Exception {
			return failCommon(msg, false, FastHomeFailure::Motion);
		};
		auto failKeepingDefault = [&](const char * msg) -> Exception {
			return failCommon(msg, false, FastHomeFailure::Backlash);
		};

		MotionProfile seekProfile;
		seekProfile.maximumSpeed = p->seekSpeed;
		seekProfile.acceleration = p->seekAccel;
		// Low enough that the final approach advances at most one microstep per main-loop
		// iteration. routineMoveTo terminates on EXACT equality (`position != targetPosition`),
		// so a floor high enough to cover more than a microstep per iteration lets the axis
		// step straight over its target and orbit it forever. The reference port used 1000
		// here, and at that floor the 1,024-microstep step-off before the backlash measurement
		// overshot, came back, and then oscillated between two positions 12 microsteps short of
		// target for the entire remaining timeout -- which surfaced as "Timeout" attributed to
		// the backlash phase, on a routine that had otherwise completed perfectly.
		//
		// 50 microsteps/s is one pulse per 20 ms, so the approach lands exactly; the creep only
		// applies to the last few microsteps, since acceleration is unchanged.
		seekProfile.minimumSpeed = 50;
		this->setMotionProfile(seekProfile);

		// Run the whole routine in raw commanded-position space, with backlash compensation
		// off from the very first move rather than switched off partway through.
		//
		// The routine does not need compensation: every datum-critical approach is deliberately
		// forward-engaged, so the gear mesh is always loaded the same way. What it does need is
		// for the position frame to mean the same thing at the end (where backlash is measured)
		// as it did at the start. Disabling this later, after moves have already been made with
		// it active, discards pending `positionWithinBacklash` state and moves the frame.
		this->backlashControl.systemBacklash = 0;
		this->backlashControl.positionWithinBacklash = 0;
		this->switchLatchDebounce = p->coarseDebounceM;

		// Three ways in, not two.
		//
		//   WARM    this axis has measured its own threshold and width already this power-up.
		//   SEEDED  it has not, but the fleet default has not been ruled out either -- so start
		//           from the default and let the routine's own gates verify it. This is the
		//           ordinary case on a production module and it skips both the background guard
		//           and the whole crossing calibration, which is where a cold run spends most of
		//           its time.
		//   COLD    the default has been tried and rejected on this axis, so measure everything.
		//
		// Seeded and warm run the same code path; they differ only in how much the width gate is
		// willing to believe (see widthMin/widthMax below) and in what a failure means -- a
		// seeded failure rejects the default and hands the next attempt to the cold path, which
		// is what makes this an optimisation rather than a restriction.
		bool warm = (this->opticalThresholdCached > 0) && (this->opticalWidthCached > 0);
		int T = warm ? this->opticalThresholdCached : 0;
		Steps W_cal = warm ? this->opticalWidthCached : 0;

		bool seeded = false;
		if(!warm && !this->opticalDefaultRejected) {
			T = FASTHOME_T_DEFAULT;
			W_cal = FASTHOME_W_DEFAULT;
			warm = true;
			seeded = true;
		}
		seededRun = seeded;

		{
			char message[90];
			sprintf(message, "fastHome begin (%s, T=%d, W_cal=%d)"
				, seeded ? "default" : (warm ? "warm" : "cold"), T, (int) W_cal);
			log(LogLevel::Status, moduleName, message);
		}

		// Phase stamps. The cold path has repeatedly run out of its overall budget in the last
		// phase, and estimating which earlier phase ate the time was guesswork twice over -- so
		// each boundary says how long it had taken by then, straight to serial.
		const uint32_t routineStart = millis();
		auto phaseStamp = [&](const char * phaseName) {
			char message[80];
			sprintf(message, "  [t+%lus] %s\r\n"
				, (unsigned long) ((millis() - routineStart) / 1000U), phaseName);
			Logger::X().printRaw(message);
		};

		// ---- Phase 0: background guard (cold runs only) ------------------------------------
		int T_cap = T; // warm: seek directly at the cached operating threshold
		if(!warm) {
			int maxBg = -1;
			int measuredCount = 0;
			for(int i = 0; i < 3; i++) {
				if(i > 0) {
					auto r = this->routineMoveTo(this->getPosition() + p->ustepsPerRev / 3, timeoutTime);
					if(r.exception) return fail("abort");
				}
				bool rail;
				int c = fastHomeSettledCrossingProbe(this->homeSwitch, rail, timeoutTime);
				if(c == -2) return fail("abort");
				// A censored probe means no crossing anywhere in the swept range. Which rail it
				// sat at says which side: `rail` is the reading at the BOTTOM of the bracket, so
				// false means "never active even at 255" -- crossing above 255, i.e. darker than
				// measurable. That is the NORMAL background reading on this ring; a measurable
				// crossing here is the reflective home flag, not the background.
				{
					char message[90];
					sprintf(message, "  bg probe %d: %s\r\n"
						, i
						, c >= 0 ? "" : (rail ? "censored, brighter than measurable" : "censored, darker than measurable"));
					if(c >= 0) {
						sprintf(message, "  bg probe %d: crossing %d\r\n", i, c);
					}
					Logger::X().printRaw(message);
				}
				if(c >= 0) {
					measuredCount++;
					if(c > maxBg) maxBg = c;
				}
			}

			// Majority, not "any". The flag is well under 1% of the circumference, so of three
			// probes spread 120 degrees apart at most ONE can be sitting on it -- which means a
			// lone measurable crossing among censored ones is the flag, and treating it as the
			// background sets T_cap just below the flag's own crossing, where the flag can never
			// read active. That is exactly what happened on the first run against this board:
			// probe 2 landed on the flag at 234, T_cap became 231, and the seek then swept a
			// whole revolution without ever seeing it ("flag not found").
			//
			// So the background is only "measurable" if at least two probes agree it is; a
			// single outlier is the flag and the background is still censored.
			const bool backgroundMeasurable = measuredCount >= 2;
			T_cap = backgroundMeasurable
				? (maxBg - FASTHOME_BG_GUARD_MARGIN)
				: FASTHOME_T_CAP_DARK;
			if(T_cap < 16) T_cap = 16;
			if(T_cap > 255) T_cap = 255;

			char message[90];
			sprintf(message, "bg guard: %s (%d/3 measurable), T_cap=%d"
				, backgroundMeasurable ? "background measurable" : "background censored"
				, measuredCount
				, T_cap);
			log(LogLevel::Status, moduleName, message);
		}
		phaseStamp("background guard done");
		// ---- Phase 1: acquire the flag, adapting the threshold if need be -------------------
		//
		// The acquisition threshold has to be found per axis, not assumed. On this board the two
		// axes want thresholds about eleven counts apart -- axis A acquires happily at 253 while
		// axis B's flag crosses at roughly the ceiling -- and there is no single constant that
		// serves both: 253 leaves axis B intermittently unable to see its flag at all, and 255
		// makes axis A read active across more than a quarter revolution.
		//
		// So start from the background guard's estimate and walk, using the two failures as the
		// signal for which way to go. Both are unambiguous:
		//   * cannot get off the flag / active over a huge span -> too permissive -> lower it
		//   * a whole revolution with no flag                   -> too strict     -> raise it
		// This is the adaptive step the port originally simplified away, on the grounds that the
		// retry loop would cover it. It does not: every retry starts from the same constant and
		// so makes the same mistake.
		// The seek's trailing edge is NOT usable, and it is worth saying why, because latching it
		// in the same pass looks free and is not.
		//
		// routineMoveToUntilSeeSwitch calls stop() the moment the leading edge latches, and a
		// stop from the 24,000 microsteps/s seek at 100,000/s^2 coasts about 2,900 microsteps.
		// Census measurements of these painted flags put them at 50-400 microsteps wide anywhere
		// in the sane part of the band, so by the time the axis is standing still it is already
		// well past the far side. Latching "the next inactive transition" from there returns the
		// deceleration distance dressed up as a flag width -- measured at 5,721 and 8,401 on an
		// axis whose flag is really 185 wide -- and every crossing probe placed across that span
		// then lands on bare ring and reports the flag as unmeasurable.
		//
		// So the span comes from a proper low-speed local re-scan below. What the acquisition
		// does hand over is the lead POSITION (which makes that re-scan short and certain) and
		// the ceiling T_cap (which bounds the crossing search).
		this->switchesArmed = true;
		Steps coarseLead = 0;
		{
			bool acquired = false;
			for(int attempt = 0; attempt <= FASTHOME_MAX_T_ADJUST && !acquired; attempt++) {
				if(millis() > timeoutTime) return fail("timeout");
				HomeSwitch::setThreshold((uint8_t) T_cap);
				HAL_Delay(FASTHOME_SETTLE_MS);

				// Starting on the flag is normal rather than an error: a successful home parks
				// the axis on its datum, and the datum is the middle of the flag, so every
				// re-home and every warm second pass begins exactly there.
				if(this->homeSwitch.getForwardsActive()) {
					const Steps exitBudget = p->ustepsPerRev / 4;
					this->inInterrupt.invertSwitches = true;   // latch becoming INACTIVE
					this->updateStepsAndSwitches();
					auto r = this->routineMoveToUntilSeeSwitch(this->getPosition() + exitBudget,
						SwitchesMask{ true, false }, timeoutTime);
					this->inInterrupt.invertSwitches = false;
					if(r.exception || !r.frameSwitchEvents.forwards.seen) {
						// Active across a quarter revolution is not a flag, it is a threshold
						// too permissive for the surface in front of it.
						if(T_cap <= 16) return fail("active everywhere, even at the floor");
						T_cap -= 2;
						char message[80];
						sprintf(message, "acquire: active >%d usteps, lowering T_cap to %d"
							, (int) exitBudget, T_cap);
						log(LogLevel::Status, moduleName, message);
						continue;
					}
					// Clear of the flag, but only just -- give the seek somewhere to accelerate
					// from so it does not immediately re-latch the edge it has only now left.
					auto clear = this->routineMoveTo(this->getPosition() + p->takeup, timeoutTime);
					if(clear.exception) return fail(clear.exception.getMessage().c_str());
				}

				auto r = this->routineMoveToUntilSeeSwitch(this->getPosition() + lapBudget,
					SwitchesMask{ true, false }, timeoutTime);
				if(!r.exception && r.frameSwitchEvents.forwards.seen) {
					coarseLead = r.frameSwitchEvents.forwards.positionSeen;
					acquired = true;
					break;
				}

				// A completed sweep that saw nothing comes back as an exception
				// ("Switch not seen"), which is the ordinary "too strict" outcome and the whole
				// reason this loop exists -- so it must not be treated as a hard error. Only an
				// operator abort or the overall deadline ends the routine here; anything else
				// means try again higher.
				if(App::getShouldEscapeFromRoutine()) return fail("abort");
				if(millis() > timeoutTime) return fail("timeout");

				if(T_cap >= 255) return fail("flag not found even at the ceiling");
				T_cap += 1;
				char message[80];
				sprintf(message, "acquire: no flag in a full revolution, raising T_cap to %d"
					, T_cap);
				log(LogLevel::Status, moduleName, message);
			}
			if(!acquired) return fail("flag not found");
		}

		{
			char message[90];
			sprintf(message, "acquired at %s=%d lead=%d", warm ? "T" : "T_cap", T_cap
				, (int) coarseLead);
			log(LogLevel::Status, moduleName, message);
		}
		phaseStamp("seek done");
		// ---- Phase 2/3: band-centred threshold calibration (cold runs only) -----------------
		// See HomeSwitchTest/reports/newring/HOME_ROUTINE_DESIGN.md. Replaces the old
		// background-margin scheme, which has no inputs on a ring whose background never
		// crosses at any threshold.
		if(!warm) {
			// Local width probe: jog to leadGuess - clearance, then latch lead+trail forward at
			// edgeSpeed within a short, bounded local search. Returns width, or -1 if not found.
			// How long to allow a constant-speed search to cover a given distance, plus slack
			// for the ramp and the routine's own per-iteration overhead.
			//
			// This replaced a flat 3-second window, which was not enough and failed in a way
			// that looked like an optical problem rather than a timing one. The probe starts a
			// full `clearance` behind the flag (5,000 microsteps at 32:1) and creeps forward at
			// edgeSpeed (2,000/s), so 2.5 s is spent just reaching the flag; a 3 s window left
			// 0.5 s of margin. As the threshold steps down the flag narrows and its leading
			// edge arrives later, so the scan started reporting "not found" partway down --
			// truncating the band and making a perfectly good flag read as band=2, i.e.
			// "insufficient optical contrast", on a board whose flag is ~1,900 microseconds
			// wide and plainly visible.
			auto boundedDeadline = [&](Steps distance, StepsPerSecond speed) -> uint32_t {
				uint32_t deadline = millis()
					+ (uint32_t)((int64_t) distance * 1000 / (int64_t) speed) + 2000;
				return deadline > timeoutTime ? timeoutTime : deadline;
			};

			// `approach` is how far behind the flag to start the creep and `speed` how fast to
			// creep. Both are parameters because this probe is used for two different jobs:
			//
			//  * locating the flag at T_cap so the crossing points can be spaced across it --
			//    not datum-critical, and the acquisition has just said exactly where the lead
			//    is, so it runs from close in and fast;
			//  * measuring W_cal at T_op, which feeds every width gate -- that one keeps the
			//    slow edgeSpeed creep, because a width latched at speed is biased by the
			//    debounce and the gates would inherit the bias.
			auto localWidthProbe = [&](Steps leadGuess, Steps approach, StepsPerSecond speed
				, Steps * outLead, Steps * outTrail) -> Steps {
				auto r = this->routineMoveTo(leadGuess - approach, timeoutTime);
				if(r.exception) return -1;
				// Budgeted against how far the flag can plausibly smear, not an operating-point
				// width (too small -- a wide flag reads as a missing one) and not a fraction of
				// a revolution (too large -- every genuinely-not-found step then burns its full
				// budget, and two of those at a quarter turn each cost ~56 s, which is what
				// pushed the first successful scan past its overall timeout during backlash).
				// The measured smear at the acquisition threshold on this board was ~1,900
				// microsteps against a coarseWidthMax of 4,200, so these are several times the
				// worst observed case while still failing fast.
				const Steps leadBudget = approach * 2 + p->coarseWidthMax;
				const Steps trailBudget = p->coarseWidthMax * 2;
				r = this->routineMoveToFindSwitch(true, speed, SwitchesMask{ true, false }
					, boundedDeadline(leadBudget, speed));
				if(r.exception || !r.frameSwitchEvents.forwards.seen) return -1;
				Steps lead = r.frameSwitchEvents.forwards.positionSeen;

				this->inInterrupt.invertSwitches = true;
				this->updateStepsAndSwitches();
				r = this->routineMoveToFindSwitch(true, speed, SwitchesMask{ true, false }
					, boundedDeadline(trailBudget, speed));
				this->inInterrupt.invertSwitches = false;
				if(r.exception || !r.frameSwitchEvents.forwards.seen) return -1;
				Steps trail = r.frameSwitchEvents.forwards.positionSeen;

				if(outLead) *outLead = lead;
				if(outTrail) *outTrail = trail;
				return trail - lead;
			};

			// Measure the flag's OWN crossing, rather than inferring a floor from where the
			// measured width collapses.
			//
			// The width-vs-threshold band scan this replaced needs the flag to be resolvable
			// over a range of thresholds several counts wide. That is true of axis A (detectable
			// from 253 all the way down to 235) and structurally impossible for axis B, whose
			// flag only resolves at 252-253 -- so the scan returned a single usable sample,
			// computed band=0, and refused. The refusal was arithmetically correct and
            // practically useless: axis B's flag is plainly there, it is just dim.
			//
			// With the background censored -- which is the measured situation on both axes here,
			// the moulding never crossing anywhere in 0..255 -- the usable threshold window is
			// simply "above the flag's crossing, at or below the safe ceiling". Both ends are
			// known directly: the crossing from one settled probe on the flag, the ceiling from
			// the background guard. That needs no width contrast at all, so it works on a
			// two-count margin as readily as on a nineteen-count one, and it costs one probe
			// instead of a ten-step scan (which is also why a cold run drops from ~100 s).
			//
			// The acquisition pass has just latched the leading edge, so this scan knows where
			// the flag is to within a debounce rather than to within a revolution. That is what
			// makes it cheap: it starts a takeup behind the KNOWN lead instead of a full
			// clearance behind a guess, and creeps at CAL_SCAN_SPEED rather than the datum
			// edgeSpeed, because nothing here feeds a datum -- the numbers are only used to
			// space crossing probes across the flag. About 0.5 s instead of 2.5 s.
			Steps capLead = 0, capTrail = 0;
			Steps spanAtCap = localWidthProbe(coarseLead, p->takeup, FASTHOME_CAL_SCAN_SPEED
				, &capLead, &capTrail);
			if(spanAtCap <= 0) {
				// A high-speed latch can be genuine yet displaced far enough that a small local
				// scan around its coordinate misses the feature. Repeating that same operation
				// (or adding motor current) contributes no evidence. Survey a bounded lap at 8k,
				// then require a complete 2k lead/trail confirmation at the recovered location.
				log(LogLevel::Warning, moduleName
					, "survey recovery: 24k acquisition disagreed with local scan; reacquiring at 8k");
				MotionProfile surveyProfile = seekProfile;
				surveyProfile.maximumSpeed = FASTHOME_CAL_SCAN_SPEED;
				this->setMotionProfile(surveyProfile);

				if(this->homeSwitch.getForwardsActive()) {
					this->inInterrupt.invertSwitches = true;
					this->updateStepsAndSwitches();
					auto exit = this->routineMoveToUntilSeeSwitch(
						this->getPosition() + p->ustepsPerRev / 4
						, SwitchesMask{ true, false }, timeoutTime);
					this->inInterrupt.invertSwitches = false;
					if(exit.exception || !exit.frameSwitchEvents.forwards.seen) {
						this->setMotionProfile(seekProfile);
						return failCommon("survey recovery: feature remains active too long", true
							, FastHomeFailure::FeatureTooWide);
					}
				}
				auto clear = this->routineMoveTo(this->getPosition() + p->takeup, timeoutTime);
				if(clear.exception) {
					this->setMotionProfile(seekProfile);
					return failMotion(clear.exception.getMessage().c_str());
				}
				auto survey = this->routineMoveToUntilSeeSwitch(
					this->getPosition() + lapBudget, SwitchesMask{ true, false }, timeoutTime);
				this->setMotionProfile(seekProfile);
				if(survey.exception || !survey.frameSwitchEvents.forwards.seen) {
					return failSpeed("survey recovery: feature absent at 8k");
				}
				coarseLead = survey.frameSwitchEvents.forwards.positionSeen;
				spanAtCap = localWidthProbe(coarseLead, p->clearance, p->edgeSpeed
					, &capLead, &capTrail);
				if(spanAtCap <= 0) {
					return failSpeed("survey recovery: 8k edge absent at 2k");
				}
				char recovered[110];
				sprintf(recovered, "survey recovery OK: T=%d lead=%d trail=%d width=%d"
					, T_cap, (int) capLead, (int) capTrail, (int) spanAtCap);
				log(LogLevel::Status, moduleName, recovered);
			}

			// Probe across the span and keep the BRIGHTEST (lowest) crossing.
			//
			// Not the midpoint. At the acquisition threshold the active span is a broad, faint
			// skirt -- about 1,900 microsteps on axis A unpainted -- while the bright core that
			// survives to the operating threshold is only a couple of hundred wide. The core is
			// not necessarily central in that skirt, so probing the midpoint alone can land on
			// background and report "no optical return" for a flag that is unmistakably there.
			// Sampling across it and taking the minimum finds the core wherever it sits, and the
			// per-point values are logged because their shape is what says whether the feature
			// is one clean reflector or something with structure.
			//
			// Only the FIRST point pays for a full binary search. Every point after it is a
			// single settled question -- "is this brighter than the best so far?" -- and only a
			// point that answers yes has to be resolved properly. On a uniform flag that is one
			// 1.5 s probe and four 0.22 s dismissals instead of five 1.5 s probes; on a flag
			// whose core sits at the far end it degrades to at worst the old cost. The
			// intervening moves are forward-only, because the points are visited in increasing
			// position order and a forward move keeps the gear mesh engaged the same way the
			// rest of the routine relies on.
			int C_flag = 256;
			Steps C_flagAt = 0;
			{
				// Approach the first point from behind, forward-engaged, exactly as before. The
				// axis is currently sitting past the trailing edge, so this is the one reversal
				// the scan needs; from here on it only ever moves forward.
				const Steps firstAt = capLead
					+ (Steps)((int64_t) spanAtCap * 1 / (FASTHOME_CROSSING_POINTS + 1));
				auto r = this->routineMoveTo(firstAt - p->clearance - p->takeup, timeoutTime);
				if(!r.exception) r = this->routineMoveTo(firstAt, timeoutTime);
				if(r.exception) return fail(r.exception.getMessage().c_str());

				for(int i = 1; i <= FASTHOME_CROSSING_POINTS; i++) {
					if(millis() > timeoutTime) return fail("timeout");
					const Steps at = capLead
						+ (Steps)((int64_t) spanAtCap * i / (FASTHOME_CROSSING_POINTS + 1));
					if(i > 1) {
						r = this->routineMoveTo(at, timeoutTime);
						if(r.exception) return fail(r.exception.getMessage().c_str());
					}

					// Cheap dismissal first. `C_flag` is the best crossing found so far, so a
					// point that is not active one count below it cannot improve on it and needs
					// no further measurement.
					if(C_flag <= FASTHOME_CAL_BRACKET_LO + 1) break; // already at the bracket floor
					if(i > 1) {
						const int brighter = fastHomeCrossingIsBrighterThan(this->homeSwitch
							, C_flag - 1, timeoutTime);
						if(brighter == -2) return fail("abort");
						if(brighter == 0) {
							char message[80];
							sprintf(message, "  probe %d @%d: not brighter than %d\r\n"
								, i, (int) at, C_flag);
							Logger::X().printRaw(message);
							continue;
						}
					}

					// Worth resolving. Search only up to the ceiling we would ever operate at
					// (the first point) or up to the incumbent best (later points).
					//
					// For a later point the top of the bracket needs no re-testing: the dismissal
					// question just settled at exactly that duty and found the sensor active, so
					// `knownActiveAtHi` is a measurement rather than an assumption. The first
					// point has no such measurement -- the acquisition latches say the flag is
					// active BETWEEN them, not at any particular position -- so it pays the one
					// extra settle and gets a real censored/not-censored answer for it.
					bool rail = false;
					const bool firstPoint = (i == 1);
					const int hi = firstPoint ? T_cap : (C_flag - 1);
					const int c = fastHomeSettledCrossingProbeBounded(this->homeSwitch, rail
						, hi, !firstPoint, timeoutTime);
					if(c == -2) return fail("abort");

					char message[100];
					sprintf(message, "  probe %d @%d: %s%d\r\n", i, (int) at
						, c >= 0 ? "crossing " : (rail ? "censored bright " : "censored dark ")
						, c);
					Logger::X().printRaw(message);

					// A bright rail means brighter than the bracket bottom, which is still a
					// perfectly good flag -- clamp rather than discard it.
					const int effective = (c >= 0) ? c : (rail ? FASTHOME_CAL_BRACKET_LO : 256);
					if(effective < C_flag) { C_flag = effective; C_flagAt = at; }
				}
			}
			if(C_flag > 255) {
				log(LogLevel::Error, moduleName
					, "no measurable crossing anywhere across the flag span");
				return failContrast("flag crossing not measurable");
			}

			const int usable = T_cap - C_flag;
			{
				char message[120];
				sprintf(message, "flag: lead=%d span@cap=%d crossing=%d@%d ceiling=%d usable=%d (min %d)"
					, (int) capLead, (int) spanAtCap, C_flag, (int) C_flagAt
					, T_cap, usable, FASTHOME_MARGIN_MIN);
				log(LogLevel::Status, moduleName, message);
			}
			if(usable < FASTHOME_MARGIN_MIN) return failContrast("insufficient optical contrast");

			int T_op = C_flag + (int) round(FASTHOME_T_OP_FRACTION * (float) usable);
			if(T_op <= C_flag) T_op = C_flag + 1;
			if(T_op > T_cap) T_op = T_cap;

			HomeSwitch::setThreshold((uint8_t) T_op);
			HAL_Delay(FASTHOME_SETTLE_MS);
			// W_cal, unlike the span above, feeds every width gate -- so it keeps the slow
			// edgeSpeed creep from a full clearance behind, the same measurement the precise
			// passes will make.
			Steps W_atOp = localWidthProbe(coarseLead, p->clearance, p->edgeSpeed, nullptr, nullptr);
			if(W_atOp < 0) return failSpeed("operating point: flag not repeatable at edge speed");

			T = T_op;
			W_cal = W_atOp;

			{
				char message[90];
				sprintf(message, "operating point: T_op=%d W_cal=%d (crossing=%d)"
					, T_op, (int) W_cal, C_flag);
				log(LogLevel::Status, moduleName, message);
			}
		}

		HomeSwitch::setThreshold((uint8_t) T);
		HAL_Delay(FASTHOME_SETTLE_MS);

		// No shoulder check here. It used to assert the sensor reads inactive at this point,
		// which is only true on a COLD run -- there the band scan's last width probe happens to
		// leave the axis past the trailing edge. A warm run skips the band scan entirely and so
		// arrives here still sitting on the leading edge the seek stopped at, reading active,
		// and failed every time with "shoulder gate: active below lead" despite nothing being
		// wrong. The check it was trying to make -- that the pass starts from clear background
		// below the flag -- is made properly in phase 4 below, after repositioning to the arming
		// point, where "inactive" is actually meaningful.

		// Width-relative gates and debounce, scaled from the calibrated flag width so they
		// auto-scale to whatever ring is actually attached (HOME_ROUTINE_DESIGN.md).
		//
		// A seeded run widens the gate, because W_cal is then a fleet constant rather than this
		// axis's own measurement and the tight gate would be judging the module against another
		// module's flag. It is still a real gate -- it rejects a phantom latch and a smeared
		// half-revolution -- and it is only this loose for the run that adopts the default; the
		// success path below caches the width actually measured, so every run after it is
		// judged against itself.
		const float widthLo = seeded ? FASTHOME_SEED_WIDTH_LO : 0.65f;
		const float widthHi = seeded ? FASTHOME_SEED_WIDTH_HI : 1.35f;
		Steps widthMin = (Steps)((float) W_cal * widthLo);
		Steps widthMax = (Steps)((float) W_cal * widthHi);
		uint16_t calibratedDebounce = (uint16_t)(W_cal / 8);
		if(calibratedDebounce < FASTHOME_DEBOUNCE_MIN) calibratedDebounce = FASTHOME_DEBOUNCE_MIN;
		if(calibratedDebounce > FASTHOME_DEBOUNCE_MAX) calibratedDebounce = FASTHOME_DEBOUNCE_MAX;
		this->switchLatchDebounce = calibratedDebounce;

		phaseStamp("calibration done");
		// ---- Phase 4: precise forward two-edge pass(es) -------------------------------------
		MotionProfile reposProfile = seekProfile;
		reposProfile.maximumSpeed = p->reposSpeed;

		// Arm the precise pass: stand a clearance BELOW the flag, forward-engaged, reading
		// inactive. Then creep up onto it.
		//
		// `clearance` is measured back from `coarseLead`, and coarseLead comes from the coarse
		// seek -- a latch taken at 24,000 microsteps/s, which is a far less certain statement
		// about where the edge is than the 2,000-microsteps/s creep that follows. Measured on
		// this module, the seek's leading edge has landed anywhere from 12 to 4,892 microsteps
		// past the creep's, so a fixed 5,000 clearance leaves margin that ranges from ample to
		// 108 microsteps -- and on one axis it went negative, i.e. the "arming point" was
		// already ON the flag and the run failed with "active at pass arming point" despite
		// nothing being wrong with the module.
		//
		// So do not assert the margin, ESTABLISH it: if the arming point reads active, step
		// back another clearance and look again. Each retry costs a fraction of a second and
		// the loop is bounded, which is a far better trade than failing a healthy axis and
		// sending it round the whole self-calibration.
		auto armForPass = [&](Steps referenceLead, Steps approach) -> Exception {
			for(int attempt = 0; attempt <= FASTHOME_ARM_BACKOFFS; attempt++) {
				const Steps armAt = referenceLead - approach - (Steps) attempt * p->clearance;
				this->setMotionProfile(reposProfile);
				auto r = this->routineMoveTo(armAt - p->takeup, timeoutTime);
				if(!r.exception) r = this->routineMoveTo(armAt, timeoutTime);
				this->setMotionProfile(seekProfile);
				if(r.exception) return fail(r.exception.getMessage().c_str());

				if(!this->homeSwitch.getForwardsActive()) {
					if(attempt > 0) {
						char message[90];
						sprintf(message, "  armed at lead-%d after %d backoff(s)\r\n"
							, (int) (referenceLead - armAt), attempt);
						Logger::X().printRaw(message);
					}
					return Exception::None();
				}
			}
			return fail("active at pass arming point");
		};

		{
			auto exception = armForPass(coarseLead, p->clearance);
			if(exception) return exception;
		}

		Steps lead = 0, trail = 0;
		Steps firstWidth = 0, firstMid = 0;
		Steps midSum = 0, widthSum = 0;
		for(int pass = 0; pass < FASTHOME_PASSES; pass++) {
			if(pass > 0) {
				// Arm off the edge the PREVIOUS pass actually measured, not off coarseLead.
				//
				// coarseLead is a 24,000-microsteps/s latch and runs measurably late -- `seekErr`
				// in the pass line below has been 124 to 4,892 microsteps on this module -- so an
				// arming point a fixed clearance behind it lands anywhere from a comfortable
				// 5,000 microsteps short of the flag to 100 microsteps short of it, or past it.
				// `lead` is a 2,000-microsteps/s creep onto the same edge, so a takeup behind it
				// is a known, small, and repeatable approach: enough to re-engage the mesh
				// forward, short enough that the creep costs a second rather than three.
				auto exception = armForPass(lead, p->takeup);
				if(exception) return exception;
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

			{
				// `seekErr` is how far past this creep's leading edge the coarse seek's latch
				// sat. It is what decides whether the arming point has any margin left, it is
				// speed- and axis-dependent, and nothing else in the log reports it.
				char message[110];
				sprintf(message, "  pass %d: lead=%d trail=%d w=%d mid=%d seekErr=%d\r\n"
					, pass, (int) lead, (int) trail, (int) (trail - lead)
					, (int) ((lead + trail) / 2), (int) (coarseLead - lead));
				Logger::X().printRaw(message);
			}

			const Steps passWidth = trail - lead;
			const Steps passMid = (lead + trail) / 2;
			if(!seeded && (passWidth < widthMin || passWidth > widthMax)) {
				// On a warm run this is exactly the >25% width-drift-from-W_cal recalibration
				// trigger in HOME_ROUTINE_DESIGN.md -- fail() clears the cache, so the next
				// attempt (the tryCount retry Routines::calibrate wraps this in) runs cold.
				char message[80];
				sprintf(message, "width gate: w=%d outside [%d..%d]"
					, (int) (trail - lead), (int) widthMin, (int) widthMax);
				log(LogLevel::Error, moduleName, message);
				return failUnstable("width drift from calibration");
			}
			if(seeded && (passWidth < FASTHOME_DEBOUNCE_MIN
				|| passWidth > p->coarseWidthMax)) {
				char message[90];
				sprintf(message, "seed width gate: w=%d outside [%d..%d]"
					, (int) passWidth, FASTHOME_DEBOUNCE_MIN, (int) p->coarseWidthMax);
				log(LogLevel::Error, moduleName, message);
				return failCommon("seed feature width implausible", true
					, FastHomeFailure::FeatureTooWide);
			}
			if(pass == 0) {
				firstWidth = passWidth;
				firstMid = passMid;
			} else if(seeded) {
				const Steps largerWidth = firstWidth > passWidth ? firstWidth : passWidth;
				const Steps widthDelta = firstWidth > passWidth
					? firstWidth - passWidth : passWidth - firstWidth;
				const Steps midDelta = firstMid > passMid
					? firstMid - passMid : passMid - firstMid;
				if((int64_t) widthDelta * 100 > (int64_t) largerWidth * FASTHOME_REPEAT_WIDTH_PCT
					|| midDelta > FASTHOME_REPEAT_MID_MAX) {
					char message[120];
					sprintf(message, "seed repeatability: dw=%d/%d (%d%% max), dmid=%d (%d max)"
						, (int) widthDelta, (int) largerWidth, FASTHOME_REPEAT_WIDTH_PCT
						, (int) midDelta, FASTHOME_REPEAT_MID_MAX);
					log(LogLevel::Error, moduleName, message);
					return failUnstable("seed feature not repeatable");
				}
			}
			midSum += (lead + trail) / 2;
			widthSum += trail - lead;
		}
		const Steps width = (Steps)(widthSum / FASTHOME_PASSES);
		const Steps home = (Steps)(midSum / FASTHOME_PASSES);

		phaseStamp("precise passes done");
		// ---- Phase 5: backlash at the trailing edge ------------------------------------------
		// Backlash compensation was already disabled at the top of the routine, deliberately --
		// see there. It must NOT be zeroed here instead: by this point the repositioning moves
		// and both precise passes have run, and if they ran with compensation active then
		// `positionWithinBacklash` holds real pending state that zeroing simply discards,
		// shifting the position frame underneath the measurement about to be taken. That is
		// what produced a backlash of -88 on a warm pass whose two edge passes had agreed with
		// each other exactly.
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
		{
			// Both raw numbers, always -- not only on failure. A backlash out of range is the
			// one gate here whose input is invisible from the result, and the bench notes say
			// the true value breathes with temperature (measured 530 to 796 across one night on
			// the same axis), so the distribution is what a campaign needs, not just the
			// outliers.
			char message[90];
			sprintf(message, "  backlash: trail=%d reenter=%d -> %d\r\n"
				, (int) trail, (int) reenter, (int) backlash);
			Logger::X().printRaw(message);
		}
		if(backlash < -FASTHOME_BACKLASH_NEG_TOL || backlash > p->backlashMax) {
			char message[80];
			sprintf(message, "backlash out of range: %d (max %d)"
				, (int) backlash, (int) p->backlashMax);
			log(LogLevel::Error, moduleName, message);
			// Not the threshold's fault: the flag was found at it and resolved cleanly by both
			// precise passes to get this far. Retry at the same operating point.
			return failKeepingDefault("backlash out of range");
		}
		if(backlash < 0) {
			char message[100];
			sprintf(message, "backlash hysteresis: measured %d, clamped to zero (limit -%d)"
				, (int) backlash, FASTHOME_BACKLASH_NEG_TOL);
			log(LogLevel::Warning, moduleName, message);
			backlash = 0;
		}

		phaseStamp("backlash done");
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
		// Cache the width this pass actually measured, not the one the run started with. On a
		// seeded run that is the difference between carrying the fleet default around all
		// session (and judging every later run against another module's flag) and adopting this
		// axis's own number the moment it has one.
		this->opticalWidthCached = width;
		this->healthStatus.homeOK = true;
		this->healthStatus.backlashOK = true;
		this->healthStatus.switchesOK = true;
		this->lastFastHomeFailure = FastHomeFailure::None;

		// The success line. `home` is the datum in the pre-shift frame, which is what a
		// repeatability campaign differences run-to-run -- after the shift above the datum
		// always reads 0, so logging the post-shift position would measure nothing. Elapsed
		// time is here because the cold path's duration against the 120 s timeout_s is a real
		// open question (see the plan's Stage 0 item 7).
		{
			char message[100];
			sprintf(message, "fastHome OK: datum=%d w=%d backlash=%d T=%d (%s, %ds)"
				, (int) home, (int) width, (int) backlash, T
				, seeded ? "default" : (warm ? "warm" : "cold")
				, (int) ((millis() - (timeoutTime - (uint32_t) settings.timeout_s * 1000U)) / 1000U));
			log(LogLevel::Status, moduleName, message);
		}

		this->setMotionProfile(normalProfile);
		endRoutine();
		return Exception::None();
	}
#endif
}
