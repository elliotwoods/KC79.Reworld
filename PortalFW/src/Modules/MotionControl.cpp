#include "MotionControl.h"
#include "Logger.h"

namespace Modules {
	//----------
	MotionControl::MotionControl(MotorDriverSettings& motorDriverSettings
		, MotorDriver& motorDriver
		, HomeSwitch& homeSwitch)
	: motorDriverSettings(motorDriverSettings)
	, motorDriver(motorDriver)
	, homeSwitch(homeSwitch)
	{
		// note the library accepts all different formats (ticks, us, hz)

		auto stepPin = digitalPinToPinName(motorDriver.getConfig().Step);
		auto timer = (TIM_TypeDef *) pinmap_peripheral(stepPin, PinMap_TIM);
		this->timer.hardwareTimer = new HardwareTimer(timer);
		this->timer.channel = STM_PIN_CHANNEL(pinmap_function(stepPin, PinMap_TIM));
		
		this->timer.hardwareTimer->setMode(this->timer.channel
			, TIMER_OUTPUT_COMPARE_PWM1
			, stepPin);
		
		this->timer.hardwareTimer->setOverflow(10000000, MICROSEC_FORMAT);
		this->timer.hardwareTimer->setCaptureCompare(this->timer.channel
			, 127
			, TimerCompareFormat_t::RESOLUTION_8B_COMPARE_FORMAT);
		this->timer.hardwareTimer->pause();

		this->enableInterrupt();
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
		if(this->motionProfile.maximumSpeed > MOTION_MAX_SPEED) {
			this->motionProfile.maximumSpeed = MOTION_MAX_SPEED;
		}

		this->updateMotion();
	}

	//----------
	void
	MotionControl::enableInterrupt()
	{
		if(this->interruptEnabed) {
			return;
		}

		// Setup the interrupt
		this->timer.hardwareTimer->attachInterrupt([this]() {
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

			if(this->position == this->targetPosition) {
				this->stop();
			}
		});

		this->interruptEnabed = true;
	}

	//----------
	void
	MotionControl::disableInterrupt()
	{
		if(this->interruptEnabed) {
			this->timer.hardwareTimer->detachInterrupt();
		}
		this->interruptEnabed = false;
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

		if(this->timer.running) {
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
		if(strcmp("move", key) == 0) {
			msgpack::DataType dataType;
			if(!msgpack::getNextDataType(stream, dataType)) {
				return false;
			}

			// If it's just an int, then we take it as targetPosition
			if(dataType == msgpack::Int32) {
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
		else if(strcmp(key, "measureBacklash") == 0) {
			// MEASURE BACKLASH

			// Expecting Nil or array of arguments
			msgpack::DataType dataType;
			if(!msgpack::getNextDataType(stream, dataType)) {
				return false;
			}

			uint8_t timeout_s = 30;
			BacklashMeasureSettings settings;

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
					timeout_s = (uint8_t) value;
				}
				if(arraySize >= 2) {
					if(!msgpack::readInt<int32_t>(stream, settings.fastMoveSpeed)) {
						return false;
					}
				}
				if(arraySize >= 3) {
					if(!msgpack::readInt<int32_t>(stream, settings.slowMoveSpeed)) {
						return false;
					}
				}
				if(arraySize >= 4) {
					if(!msgpack::readInt<int32_t>(stream, settings.backOffDistance)) {
						return false;
					}
				}
				if(arraySize >= 5) {
					if(!msgpack::readInt<int32_t>(stream, settings.debounceDistance)) {
						return false;
					}
				}
			}

			auto exception = this->measureBacklashRoutine(timeout_s, settings);
			if(exception) {
				log(LogLevel::Error, exception.what());
				return false;
			}
			
			return true;
		}
		return false;
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
	// ReMarkable August 2023 page 2
	Exception
	MotionControl::measureBacklashRoutine(uint8_t timeout_s
			, const BacklashMeasureSettings& settings)
	{
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

		// TODO : move to seperate function
		auto microStepsPerStep = Steps(1 << (uint32_t) (uint8_t) this->motorDriverSettings.getMicrostepResolution());
		auto microstepsPerPrismRotation = Steps(MOTION_STEPS_PER_PRISM_ROTATION)
		 	* microStepsPerStep;

		// Remove main interrupt and add our own
		this->disableInterrupt();
		this->timer.hardwareTimer->attachInterrupt([this, &homeSwitch, &switchSeen]() {
			if(this->currentMotionState.direction) {
				this->position++;
			}
			else {
				this->position--;
			}

			if(this->position == this->targetPosition) {
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
		uint32_t timeoutTime = startTime + (uint32_t) timeout_s * 1000U;

		log(LogLevel::Status, "BLC : begin");

		auto endRoutine = [this]() {
			this->stop();
			this->timer.hardwareTimer->detachInterrupt();
			this->enableInterrupt();
		};

		// https://paper.dropbox.com/doc/KC79-Firmware-development-log--B9ww1dZ58Y0lrKt6fzBa9O8yAg-NaTWt2IkZT4ykJZeMERKP#:h2=Backlash-measure-algorithm
		Steps backlashSize;
		{
			// Prime the switches
			{
				switchSeen = {0};
			}

			// (1) Fast step until we're on the switch is active (ok if it's already active)
			log(LogLevel::Status, "BLC 1: fast find switch");
			if(!homeSwitch.getForwardsActive()) {
				this->run(true, settings.fastMoveSpeed);
				while(!switchSeen.seenPressed) {
					if (millis() > timeoutTime) { endRoutine(); return Exception::Timeout(); }
				}
				this->stop();
			}

			auto positionSwitchRough = switchSeen.positionFirstPressed;

			// (2) Back off the switch completely so we can approach again slowly
			log(LogLevel::Status, "BLC 2: back off switch");
			{
				this->run(false, settings.fastMoveSpeed);
				while(this->position > positionSwitchRough - settings.backOffDistance * microStepsPerStep) {
					if (millis() > timeoutTime) { endRoutine(); return Exception::Timeout(); }
				}
				this->stop();
			}

			// Prime the switches
			{
				switchSeen = {0};
			}

			// (3) Walk up slowly to find exact button start
			log(LogLevel::Status, "BLC 3: find switch again");
			{
				this->run(true, settings.slowMoveSpeed);
				while(!switchSeen.seenPressed) {
					if (millis() > timeoutTime) { endRoutine(); return Exception::Timeout(); }
				}
				this->stop();
			}

			auto postitionSwitchAccurate = switchSeen.positionFirstPressed;

			// (4) Walk forward N steps for debouncing
			log(LogLevel::Status, "BLC 4: walk into switch (debounce)");
			{
				this->targetPosition = postitionSwitchAccurate + settings.debounceDistance * microStepsPerStep;
				
				while(this->position != this->targetPosition) {
					if (millis() > timeoutTime) { endRoutine(); return Exception::Timeout(); }
					this->updateMotion();
					HAL_Delay(1);
				}
			}
			if(!this->homeSwitch.getForwardsActive()) {
				return Exception("BLC Debounce error");
			}

			// Prime the switches
			{
				switchSeen = {0};
			}

			// (5) Walk backwards slowly until button de-presses
			log(LogLevel::Status, "BLC 5: back off to find backlash");
			{
				this->run(false, settings.slowMoveSpeed);
				while(!switchSeen.seenNotPressed) {
					if (millis() > timeoutTime) { endRoutine(); return Exception::Timeout(); }
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
				, "Backlash = %d (%d/10 degrees )"
				, backlashSize
				, (int) (backlashInDegrees * 10));
			log(LogLevel::Status, message);
		}
		
		this->backlashControl.systemBacklash = backlashSize;
		
		endRoutine();
		return Exception::None();
	}
}