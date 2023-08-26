#include "MotionControl.h"
#include "Logger.h"
#include "App.h"

namespace Modules {
	//----------
	MotionControl::MotionControl(MotorDriverSettings& motorDriverSettings
		, MotorDriver& motorDriver
		, HomeSwitch& homeSwitch)
	: motorDriverSettings(motorDriverSettings)
	, motorDriver(motorDriver)
	, homeSwitch(homeSwitch)
	{
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
	void
	MotionControl::update()
	{
		this->updateStepCount();

		if(this->motionProfile.maximumSpeed > MOTION_MAX_SPEED) {
			this->motionProfile.maximumSpeed = MOTION_MAX_SPEED;
		}
		this->updateMotion();
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

		log(LogLevel::Status, "Test begin");

		do {
			HAL_Delay(10);

			// Print message
			{
				char message[100];
				sprintf(message, "%d->%d (%d)\n"
					, (int) currentCount
					, (int) target_count
					, (int) period_us);
				log(LogLevel::Status, message);
			}
		} while (currentCount < target_count);

		log(LogLevel::Status, "Test end");
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
		if(!this->timer.hardwareTimer) {
			return;
		}

		if(!this->interruptEnabed) {
			return;
		}
		this->timer.hardwareTimer->detachInterrupt();
		this->interruptEnabed = false;
	}

	//----------
	void
	MotionControl::enableInterrupt()
	{
		if(this->interruptEnabed) {
			return;
		}

		if(!this->timer.hardwareTimer) {
			return;
		}

		// Setup the interrupt
		this->timer.hardwareTimer->attachInterrupt([this]() {
			this->stepsInInterrupt++;
		});

		this->interruptEnabed = true;
	}

	//----------
	void
	MotionControl::attachCustomInterrupt(const std::function<void()>& action)
	{
		this->timer.hardwareTimer->attachInterrupt(action);
	}

	//----------
	void
	MotionControl::disableCustomInterrupt()
	{
		this->timer.hardwareTimer->detachInterrupt();
	}

	//----------
	void
	MotionControl::attachSwitchesSeenInterrupt(SwitchesSeen& switchesSeen)
	{
		this->timer.hardwareTimer->attachInterrupt([&]() {
			if(this->currentMotionState.direction) {
				this->position++;
			}
			else {
				this->position--;
			}

			if(!switchesSeen.forwards.seen) {
				if (this->homeSwitch.getForwardsActive()) {
					switchesSeen.forwards.seen = true;
					switchesSeen.forwards.positionFirstSeen = this->position;
				}
			}

			if(!switchesSeen.backwards.seen) {
				if (this->homeSwitch.getBackwardsActive()) {
					switchesSeen.backwards.seen = true;
					switchesSeen.backwards.positionFirstSeen = this->position;
				}
			}
		});
	}

	//----------
	void
	MotionControl::disableSwitchesSeenInterrupt()
	{
		this->disableCustomInterrupt();
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
	}

	//----------
	Steps
	MotionControl::getTargetPosition() const
	{
		return this->targetPosition;
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
		return (MOTION_STEPS_PER_PRISM_ROTATION) * this->motorDriverSettings.getMicrostepsPerStep();
	}

	//----------
	void
	MotionControl::zeroCurrentPosition()
	{
		this->setCurrentPosition(0);
	}

	//----------
	void
	MotionControl::setCurrentPosition(Steps steps)
	{
		this->position = steps;
		this->targetPosition = steps;
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
		this->targetPosition = this->position;
	}

	//----------
	void
	MotionControl::run(bool direction, StepsPerSecond speed)
	{
		// If no hardware timer then nothing to do here
		if(!this->timer.hardwareTimer) {
			return;
		}

		// Reset the watchdog for steps
		if(!this->currentMotionState.motorRunning) {
			this->lastStepDetectedOrRunStart = millis();
		}

		// Check minimum speed
		if(speed < this->motionProfile.minimumSpeed) {
			speed = this->motionProfile.minimumSpeed;
		}

		// Watchdog for no steps detected
		// if(millis() - this->lastStepDetectedOrRunStart > this->maxTimeWithoutSteps) {
		// 	// The timer might have crashed. We see this occasionally on B
		// 	if(this->stepWatchdogResetCount >= this->maxStepWatchdogResets) {
		// 		// System reset in this case
		// 		log(LogLevel::Error, "Steps Watchdog timed out too many time - rebooting");
		// 		NVIC_SystemReset();
		// 	}

		// 	log(LogLevel::Error, "Steps Watchdog timed out");
		// 	this->deinitTimer();
		// 	this->initTimer();
		// 	this->stepWatchdogResetCount++;

		// 	this->lastStepDetectedOrRunStart = millis();
		// }

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
					log(LogLevel::Error, exception.what());
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
					log(LogLevel::Error, exception.what());
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
					log(LogLevel::Error, exception.what());
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
					log(LogLevel::Error, exception.what());
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
		return false;
	}

	//----------
	void
	MotionControl::updateStepCount()
	{
		if(this->stepsInInterrupt > 0) {
			// Reset this watchdog
			this->lastStepDetectedOrRunStart = millis();
			this->stepWatchdogResetCount = 0;
		}

		if(this->currentMotionState.direction) {

			// Forwards

			if(this->backlashControl.positionWithinBacklash < 0) {
				// Within backlash region
				auto stepsMovedInBacklash = min(this->stepsInInterrupt, -this->backlashControl.positionWithinBacklash);
				this->backlashControl.positionWithinBacklash += stepsMovedInBacklash;
				this->stepsInInterrupt -= stepsMovedInBacklash;
			}
			
			this->position += this->stepsInInterrupt;
			this->stepsInInterrupt = 0;
		}
		else {
			// Backwards

			if(this->backlashControl.positionWithinBacklash > 0) {
				// Within backlash region
				auto stepsMovedInBacklash = min(this->stepsInInterrupt, this->backlashControl.positionWithinBacklash);
				this->backlashControl.positionWithinBacklash -= stepsMovedInBacklash;
				this->stepsInInterrupt -= stepsMovedInBacklash;
			}

			this->position -= this->stepsInInterrupt;
			this->stepsInInterrupt = 0;
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

		auto motionState = this->calculateMotionState(dt);

		if(this->musicalMode) {
			// quantise to notes
			float note = (float) motionState.speed / 262.0f;
			auto octave = (int32_t) log2f((float) note);

			// quantise to quarters
			auto octaveQuantised = ceilf(octave * 4.0f) / 4.0f;
			int32_t musicalSpeed = pow(2, octaveQuantised) * 262.0f;

			motionState.speed = musicalSpeed;
		}

		if(!motionState.motorRunning && this->currentMotionState.motorRunning) {
			// Stop all motion
			this->stop();
		}

		if(motionState.motorRunning) {
			// Run the motion
			this->run(motionState.direction, motionState.speed);
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
		StepsPerSecond maxDeltaV = this->motionProfile.acceleration * dt_us / 1000000;

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
		// Stop any existing motion profile
		this->stop();

		if(!this->timer.hardwareTimer) {
			return Exception("No hardware timer");
		}

		log(LogLevel::Status, "unjam : begin");

		// Store priors for later
		auto priorCurrent = this->motorDriverSettings.getCurrent();
		auto priorMicrostep = this->motorDriverSettings.getMicrostepResolution();

		// We won't be calibrated any more after this
		this->homeCalibrated = false;

		// Set the current to max and steps to 1
		this->motorDriverSettings.setCurrent(MOTORDRIVERSETTINGS_MAX_CURRENT);
		this->motorDriverSettings.setMicrostepResolution(MotorDriverSettings::MicrostepResolution::_1);

		// Tone down acceleration and velocity to match full steps movement
		auto priorMotionProfile = this->motionProfile;
		MotionProfile unblockMotionProfile;
		{
			unblockMotionProfile.acceleration = 500;
			unblockMotionProfile.maximumSpeed = 523; // note C5
			unblockMotionProfile.minimumSpeed = 100;
		}
		this->setMotionProfile(unblockMotionProfile);

		// Attach the switches seen interrupt
		this->disableInterrupt();
		SwitchesSeen switchesSeen;
		this->attachSwitchesSeenInterrupt(switchesSeen);

		// Start measuring time for timeout
		uint32_t startTime = millis();
		uint32_t routineDeadline = startTime + (uint32_t) settings.timeout_s * 1000U;

		auto endRoutine = [&]() {
			this->stop();
			this->setMotionProfile(priorMotionProfile);
			this->motorDriverSettings.setCurrent(priorCurrent);
			this->motorDriverSettings.setMicrostepResolution(priorMicrostep);
			this->disableSwitchesSeenInterrupt();
			this->enableInterrupt();
			log(LogLevel::Status, "unjam : end");
		};
		
		// Set end position to be 2x complete rotation
		auto endPosition = this->getMicrostepsPerPrismRotation() * 2;
		this->setTargetPosition(endPosition);
		
		// Wait for move
		log(LogLevel::Status, "unjam : Walk CW");
		while (this->getPosition() < this->getTargetPosition())
		{
			this->update();
			if(App::updateFromRoutine()) { endRoutine(); return Exception::Escape(); }
			HAL_Delay(20); // longer delay because dt is otherwise too short for these steps

			if (millis() > routineDeadline)
			{
				endRoutine();
				return Exception::Timeout();
			}
		}

		if(!switchesSeen.forwards.seen) {
			endRoutine();
			return Exception("Didn't see FW switch");
		}

		// Settle before return - seems we have issue otherwise
		for(int i=0; i<100; i++) {
			this->update();
			if(App::updateFromRoutine()) { endRoutine(); return Exception::Escape(); }
			HAL_Delay(10); // longer delay because dt is otherwise too short for these steps
		}

		// Instruct move back to 0
		this->setTargetPosition(0);

		// Wait for move
		log(LogLevel::Status, "unjam : Walk CCW");
		while (this->getPosition() > this->getTargetPosition())
		{
			this->update();
			if(App::updateFromRoutine()) { endRoutine(); return Exception::Escape(); }
			HAL_Delay(20); // longer delay because dt is otherwise too short for these steps

			if (millis() > routineDeadline)
			{
				endRoutine();
				return Exception::Timeout();
			}
		}

		if(!switchesSeen.backwards.seen) {
			endRoutine();
			return Exception("Didn't see BW switch");
		}

		endRoutine();

		return Exception::None();
	}

	//----------
	Exception
	MotionControl::tuneCurrentRoutine(const MeasureRoutineSettings& settings)
	{
		log(LogLevel::Status, "tune : begin");

		auto current = MOTORDRIVERSETTINGS_DEFAULT_CURRENT;
		this->motorDriverSettings.setCurrent(current);

		SwitchesSeen switchesSeen;
		this->disableInterrupt();
		this->attachSwitchesSeenInterrupt(switchesSeen);

		auto endRoutine = [&]() {
			this->disableSwitchesSeenInterrupt();
			this->enableInterrupt();
			log(LogLevel::Status, "tune : end");
		};

		// Start measuring time for timeout
		uint32_t startTime = millis();
		uint32_t timeoutTime = startTime + (uint32_t) settings.timeout_s * 1000U;

		// Move off switch if already on
		if(this->homeSwitch.getForwardsActive()) {
			this->setTargetPosition(this->getPosition() + this->getMicrostepsPerPrismRotation());
			log(LogLevel::Status, "tune : Move off switch");
			while(this->homeSwitch.getForwardsActive()) {
				if(millis() > timeoutTime) { endRoutine(); return Exception::Timeout(); }
				this->update();
				if(App::updateFromRoutine()) { endRoutine(); return Exception::Escape(); }
				HAL_Delay(1);
			}
			this->stop();
		}
		

		while(true) {
			log(LogLevel::Status, "tune : Single rotation CW");
			
			// Try to move and see the switch
			auto startPosition = this->position;
			auto endPosition = startPosition + this->getMicrostepsPerPrismRotation() * 11 / 10;

			this->setTargetPosition(endPosition);

			// while moving to target
			while(this->getTargetPosition() > this->getPosition()) {
				if(millis() > timeoutTime) { endRoutine(); return Exception::Timeout(); }
				this->update();
				if(App::updateFromRoutine()) { endRoutine(); return Exception::Escape(); }
				HAL_Delay(1);

				if(switchesSeen.forwards.seen) {
					log(LogLevel::Status, "tune : Switch seen");
					break;
				}
			}

			this->stop();

			if(switchesSeen.forwards.seen) {
				break;
			}
			else {
				current += 0.05f;
				if(current > MOTORDRIVERSETTINGS_MAX_CURRENT) {
					return Exception("tune : Cannot raise the current higher");
				}
				else {
					this->motorDriverSettings.setCurrent(current);
				}
			}
		}

		// Walk back onto switch
		{
			auto target = switchesSeen.forwards.positionFirstSeen;

			{
				char message[100];
				sprintf(message, "tune : Walk back onto switch at %d", target);
				log(LogLevel::Status, message);
			}

			this->setTargetPosition(target);

			while(abs(this->getPosition() - this->getTargetPosition()) > 5) {
				this->update();
				if(App::updateFromRoutine()) { endRoutine(); return Exception::Escape(); }
				HAL_Delay(1);
			}

			// Set this as new home
			this->zeroCurrentPosition();
		}

		endRoutine();
		return Exception::None();
	}

	//----------
	// ReMarkable August 2023 page 2
	Exception
	MotionControl::measureBacklashRoutine(const MeasureRoutineSettings& settings)
	{
		if(!this->timer.hardwareTimer) {
			return Exception("No hardware timer");
		}

		// Stop any existing motion profile
		this->stop();

		struct SwitchSeen {
			bool seenPressed;
			Steps positionFirstPressed;
			bool seenNotPressed;
			Steps positionFirstNotPressed;
		};
		SwitchSeen switchSeen;
		auto & homeSwitch = this->homeSwitch;

		/// Note that we need to switch this on/off depending on if we're using run or updateMotion.
		// e.g. if we just use 'run' and aren't interested in the targetPosition, then this might cause a premature stop
		bool motionControlInterruptStopEnabled = false;

		auto microStepsPerStep = this->motorDriverSettings.getMicrostepsPerStep();
		const auto microstepsPerPrismRotation = this->getMicrostepsPerPrismRotation();

		// Remove main interrupt and add our own
		this->disableInterrupt();
		this->timer.hardwareTimer->attachInterrupt([&]() {
			if(this->currentMotionState.direction) {
				this->position++;
			}
			else {
				this->position--;
			}

			if(motionControlInterruptStopEnabled && this->position == this->targetPosition) {
				this->stop();
			}

			if(!switchSeen.seenNotPressed) {
				if(!homeSwitch.getForwardsActive()) {
					switchSeen.seenNotPressed = true;
					switchSeen.positionFirstNotPressed = this->position;
				}
			}

			if(!switchSeen.seenPressed) {
				if(homeSwitch.getForwardsActive()) {
					switchSeen.seenPressed = true;
					switchSeen.positionFirstPressed = this->position;
				}
			}
		});

		HAL_Delay(10);

		// Start measuring time for timeout
		uint32_t startTime = millis();
		uint32_t timeoutTime = startTime + (uint32_t) settings.timeout_s * 1000U;

		log(LogLevel::Status, "BLC : begin");

		auto endRoutine = [this]() {
			this->stop();
			this->timer.hardwareTimer->detachInterrupt();
			this->enableInterrupt();
			log(LogLevel::Status, "BLC : end");
		};

		// https://paper.dropbox.com/doc/KC79-Firmware-development-log--B9ww1dZ58Y0lrKt6fzBa9O8yAg-NaTWt2IkZT4ykJZeMERKP#:h2=Backlash-measure-algorithm
		Steps backlashSize;
		{
			motionControlInterruptStopEnabled = true;

			// (1) Walk off the back switch
			log(LogLevel::Status, "BLC 1: walk back off the back switch");
			{
				if(homeSwitch.getBackwardsActive()) {
					//Move backwards by clearance distance
					this->targetPosition = this->position - 50000 / 128 * microStepsPerStep;
					while(this->targetPosition != this->position) {
						if (millis() > timeoutTime) { endRoutine(); return Exception::Timeout(); }
						this->updateMotion();
						if(App::updateFromRoutine()) { endRoutine(); return Exception::Escape(); }
						HAL_Delay(1);
					}

					this->stop();
				}
			}

			motionControlInterruptStopEnabled = true;

			// Prime the switches
			{
				switchSeen = {0};
			}

			// (2) Fast step until we're on the switch is active (ok if it's already active)
			log(LogLevel::Status, "BLC 2: fast find switch");
			if(!homeSwitch.getForwardsActive()) {
				this->targetPosition = this->position + microstepsPerPrismRotation;
				while(!switchSeen.seenPressed && this->targetPosition != this->position) {
					if (millis() > timeoutTime) { endRoutine(); return Exception::Timeout(); }
					this->updateMotion();
					if(App::updateFromRoutine()) { endRoutine(); return Exception::Escape(); }
					HAL_Delay(1);
				}
				this->stop();

				if(this->position == this->targetPosition && !switchSeen.seenPressed) {
					return Exception("Couldn't find switch");
				}
			}

			auto positionSwitchRough = switchSeen.positionFirstPressed;

			// (3) Back off the switch completely so we can approach again slowly
			{
				log(LogLevel::Status, "BLC 3: back off switch and push past backlash size");

				auto backOffPosition = positionSwitchRough - settings.backOffDistance * microStepsPerStep;
				auto backOffPlusClearBacklashPosition = backOffPosition - 30000 * microStepsPerStep / 128;

				// back way off
				{
					this->targetPosition = backOffPlusClearBacklashPosition;
					while(this->targetPosition != this->position) {
						if (millis() > timeoutTime) { endRoutine(); return Exception::Timeout(); }
						this->updateMotion();
						if(App::updateFromRoutine()) { endRoutine(); return Exception::Escape(); }
						HAL_Delay(1);
					}
				}

				// back off close
				{
					this->targetPosition = backOffPosition;
					while(this->targetPosition != this->position) {
						if (millis() > timeoutTime) { endRoutine(); return Exception::Timeout(); }
						this->updateMotion();
						if(App::updateFromRoutine()) { endRoutine(); return Exception::Escape(); }
						HAL_Delay(1);
					}
				}
			}
			

			// Prime the switches
			{
				switchSeen = {0};
			}

			// (4) Walk up slowly to find exact button start
			log(LogLevel::Status, "BLC 4: find switch again");
			{
				this->run(true, settings.slowMoveSpeed);
				while(!switchSeen.seenPressed) {
					if (millis() > timeoutTime) { endRoutine(); return Exception::Timeout(); }
					if(App::updateFromRoutine()) { endRoutine(); return Exception::Escape(); }
					HAL_Delay(1);
				}
				this->stop();
			}

			motionControlInterruptStopEnabled = true;

			auto postitionSwitchAccurate = switchSeen.positionFirstPressed;

			HAL_Delay(500);

			// (5) Walk forward N steps for debouncing
			log(LogLevel::Status, "BLC 5: walk into switch (debounce)");
			{
				this->targetPosition = postitionSwitchAccurate + settings.debounceDistance * microStepsPerStep;
				
				while(this->position != this->targetPosition) {
					if (millis() > timeoutTime) { endRoutine(); return Exception::Timeout(); }
					this->updateMotion();
					if(App::updateFromRoutine()) { endRoutine(); return Exception::Escape(); }
					HAL_Delay(1);
				}
			}
			if(!this->homeSwitch.getForwardsActive()) {
				endRoutine();
				return Exception("BLC Debounce error");
			}

			// Prime the switches
			{
				switchSeen = {0};
			}

			motionControlInterruptStopEnabled = false;

			// (6) Walk backwards slowly until button de-presses
			log(LogLevel::Status, "BLC 6: back off to find backlash");
			{
				this->run(false, settings.slowMoveSpeed);
				while(!switchSeen.seenNotPressed) {
					if (millis() > timeoutTime) { endRoutine(); return Exception::Timeout(); }
					if(App::updateFromRoutine()) { endRoutine(); return Exception::Escape(); }
					HAL_Delay(1);
				}
				this->stop();
			}

			// This is the position it unpresses if we walk backwards
			auto backOffPosition = switchSeen.positionFirstNotPressed;

			backlashSize = postitionSwitchAccurate - backOffPosition;
		}
			
		// Measure the current position at end of sequence
		{
			auto backlashInDegrees = 360.0f * (float) backlashSize / (float) microstepsPerPrismRotation;
			char message[100];
			sprintf(message
				, "Backlash = %d (%d/10 degrees)"
				, backlashSize
				, (int) (backlashInDegrees * 10));
			log(LogLevel::Status, message);
		}
		
		this->backlashControl.systemBacklash = backlashSize;
		this->backlashControl.backlashCalibrated = true;

		endRoutine();
		return Exception::None();
	}

	//----------
	Exception
	MotionControl::homeRoutine(const MeasureRoutineSettings& settings)
	{
		// Stop any existing motion profile
		this->stop();

		if(!this->timer.hardwareTimer) {
			return Exception("No hardware timer");
		}

		struct SwitchSeen {
			bool seenPressed;
			Steps positionFirstPressed;
		};
		SwitchSeen switchesSeen[2];

		/// Note that we need to switch this on/off depending on if we're using run or updateMotion.
		// e.g. if we just use 'run' and aren't interested in the targetPosition, then this might cause a premature stop
		bool motionControlInterruptStopEnabled = false;

		auto & homeSwitch = this->homeSwitch;

		auto microStepsPerStep = this->motorDriverSettings.getMicrostepsPerStep();
		const auto microstepsPerPrismRotation = this->getMicrostepsPerPrismRotation();

		// Remove main interrupt and add our own
		this->disableInterrupt();
		this->timer.hardwareTimer->attachInterrupt([&]() {
			if(this->currentMotionState.direction) {
				// Forwards

				if(this->backlashControl.positionWithinBacklash >= 0) {
					// Not inside backlash region
					this->position++;
				}
				else {
					// Inside backlash region
					this->backlashControl.positionWithinBacklash++;
				}
			}
			else {
				// Backwards

				if(this->backlashControl.positionWithinBacklash <= 0) {
					// Not inside backlash region
					this->position--;
				}
				else {
					// Inside backlash region
					this->backlashControl.positionWithinBacklash--;
				}
			}

			if(motionControlInterruptStopEnabled && this->position == this->targetPosition) {
				this->stop();
			}

			if(!switchesSeen[0].seenPressed) {
				if(homeSwitch.getForwardsActive()) {
					switchesSeen[0].seenPressed = true;
					switchesSeen[0].positionFirstPressed = this->position;
				}
			}

			if(!switchesSeen[1].seenPressed) {
				if(homeSwitch.getBackwardsActive()) {
					switchesSeen[1].seenPressed = true;
					switchesSeen[1].positionFirstPressed = this->position;
				}
			}
		});

		HAL_Delay(10);

		// Start measuring time for timeout
		uint32_t startTime = millis();
		uint32_t timeoutTime = startTime + (uint32_t) settings.timeout_s * 1000U;

		log(LogLevel::Status, "home : begin");

		auto endRoutine = [this]() {
			this->stop();
			this->timer.hardwareTimer->detachInterrupt();
			this->enableInterrupt();
			log(LogLevel::Status, "home : end");
		};

		const Steps buttonClearDistance = 20000 / 128 * microStepsPerStep; // here we have a value by trial and error (at 128 microsteps)

		// https://paper.dropbox.com/doc/KC79-Firmware-development-log--B9ww1dZ58Y0lrKt6fzBa9O8yAg-NaTWt2IkZT4ykJZeMERKP#:uid=201211977543731617580121&h2=Home-sequence
		Steps homePosition;
		{
			motionControlInterruptStopEnabled = true;

			// (0) Walk to where we last saw home
			log(LogLevel::Status, "Home 0: walk to last home");
			if(!this->homeSwitch.getForwardsActive() && !this->homeSwitch.getBackwardsActive()) {
				if(this->position != 0) {
					this->targetPosition = 0;
					while(this->targetPosition != this->position) {
						if (millis() > timeoutTime) { endRoutine(); return Exception::Timeout(); }
						this->updateMotion();
						if(App::updateFromRoutine()) { endRoutine(); return Exception::Escape(); }
						HAL_Delay(1);
					}

					this->stop();
				}
			}

			// (1) Walk off the back switch
			log(LogLevel::Status, "Home 1: walk back off the back switch");
			{
				if(homeSwitch.getBackwardsActive()) {
					//Move backwards by clearance distance
					this->targetPosition = this->position - buttonClearDistance;
					while(this->targetPosition != this->position) {
						if (millis() > timeoutTime) { endRoutine(); return Exception::Timeout(); }
						this->updateMotion();
						if(App::updateFromRoutine()) { endRoutine(); return Exception::Escape(); }
						HAL_Delay(1);
					}

					this->stop();
				}
			}

			// Prime the switches
			{
				switchesSeen[0] = {0};
				switchesSeen[1] = {0};
			}

			// (2) Fast step until we're on the forward switch is active (ok if it's already active)
			log(LogLevel::Status, "Home 2: fast find switch");
			if(!homeSwitch.getForwardsActive()) {
				//Move forward by one prism rotation
				this->targetPosition = this->position + microstepsPerPrismRotation;
				while(!switchesSeen[0].seenPressed) {
					if (millis() > timeoutTime) { endRoutine(); return Exception::Timeout(); }
					this->updateMotion();
					if(App::updateFromRoutine()) { endRoutine(); return Exception::Escape(); }
					HAL_Delay(1);
				}

				this->stop();
			}

			auto positionForwardSwitchRough = switchesSeen[0].positionFirstPressed;

			// (3) Back off the switch completely so we can approach again slowly
			log(LogLevel::Status, "Home 3: back off switch");
			{
				this->targetPosition = positionForwardSwitchRough
					- settings.backOffDistance * microStepsPerStep;
				
				while(this->position != this->targetPosition) {
					if (millis() > timeoutTime) { endRoutine(); return Exception::Timeout(); }
					this->updateMotion();
					if(App::updateFromRoutine()) { endRoutine(); return Exception::Escape(); }
					HAL_Delay(1);
				}
			}

			motionControlInterruptStopEnabled = false;

			// Prime the switches
			{
				switchesSeen[0] = {0};
				switchesSeen[1] = {0};
			}

			// (4) Walk up slowly to find exact button start
			log(LogLevel::Status, "Home 4: find switch slowly");
			{
				this->run(true, settings.slowMoveSpeed);
				while(!switchesSeen[0].seenPressed) {
					if (millis() > timeoutTime) { endRoutine(); return Exception::Timeout(); }
					if(App::updateFromRoutine()) { endRoutine(); return Exception::Escape(); }
					HAL_Delay(1);
				}
				this->stop();
			}

			auto positionForwardSwitchAccurate = switchesSeen[0].positionFirstPressed;

			motionControlInterruptStopEnabled = true;

			// (5) Walk forward enough to clear switch
			log(LogLevel::Status, "Home 5: walk more beyond switch to clear backlash");
			{
				this->targetPosition = positionForwardSwitchAccurate + buttonClearDistance;
				
				while(this->position != this->targetPosition) {
					if (millis() > timeoutTime) { endRoutine(); return Exception::Timeout(); }
					this->updateMotion();
					if(App::updateFromRoutine()) { endRoutine(); return Exception::Escape(); }
					HAL_Delay(1);
				}
			}

			motionControlInterruptStopEnabled = false;

			// Prime the switches
			{
				switchesSeen[0] = {0};
				switchesSeen[1] = {0};
			}

			// (6) Slow move back onto switch to find reverse switch position
			log(LogLevel::Status, "Home 6: slow move onto reverse switch");
			{
				this->run(false, settings.slowMoveSpeed);
				while(!switchesSeen[1].seenPressed) {
					if (millis() > timeoutTime) { endRoutine(); return Exception::Timeout(); }
					if(App::updateFromRoutine()) { endRoutine(); return Exception::Escape(); }
					HAL_Delay(1);
				}
				this->stop();
			}

			auto positionReverseSwitchAccurate = switchesSeen[1].positionFirstPressed;

			// Now:
			// * We're moving backward
			// * We are outisde of backlash region (since we did actually move)
			// * We had backlash control on in the interrupt, so should be already backlash corrected
			homePosition = (positionForwardSwitchAccurate + positionForwardSwitchAccurate) / 2;
		}
			
		// Measure the current position at end of sequence
		{
			auto homePositionInDegrees = 360.0f * (float) homePosition / (float) microstepsPerPrismRotation;
			char message[100];
			sprintf(message
				, "Home = %d (%d/10 degrees )"
				, homePosition
				, (int) (homePositionInDegrees * 10));
			log(LogLevel::Status, message);
		}
		
		this->position -= homePosition;
		this->targetPosition = 0;
		this->homeCalibrated = true;

		endRoutine();
		return Exception::None();
	}

	//----------
	void
	MotionControl::reportStatus(msgpack::Serializer& serializer)
	{
		serializer.beginMap(4);
		{
			serializer << "position" << this->position;
			serializer << "targetPosition" << this->targetPosition;
			serializer << "backlashCalibrated" << this->backlashControl.backlashCalibrated;
			serializer << "homeCalibrated" << this->homeCalibrated;
		}
	}

	//----------
	bool
	MotionControl::isBacklashCalibrated() const
	{
		return this->backlashControl.backlashCalibrated;
	}

	//----------
	bool
	MotionControl::isHomeCalibrated() const
	{
		return this->homeCalibrated;
	}
}