#include "MotionControl.h"
#include "Logger.h"

namespace Modules {
	//----------
	MotionControl::MotionControl(MotorDriver& motorDriver, HomeSwitch& homeSwitch)
	: motorDriver(motorDriver)
	, homeSwitch(homeSwitch)
	{
		// note the library accepts all different formats (ticks, us, hz)

		auto stepPin = digitalPinToPinName(motorDriver.getConfig().Step);
		auto timer = (TIM_TypeDef *) pinmap_peripheral(stepPin, PinMap_TIM);
		this->timer.hardwareTimer = new HardwareTimer(timer);
		this->timer.channel = STM_PIN_CHANNEL(pinmap_function(stepPin, PinMap_TIM));
		
		this->timer.hardwareTimer->setMode(this->timer.channel
			, TIMER_OUTPUT_COMPARE_PWM1, stepPin);
		
		this->timer.hardwareTimer->setOverflow(10000, MICROSEC_FORMAT);
		this->timer.hardwareTimer->setCaptureCompare(this->timer.channel
			, 127
			, TimerCompareFormat_t::RESOLUTION_8B_COMPARE_FORMAT);


		// Setup the interrupt
		this->timer.hardwareTimer->attachInterrupt([this]() {
			this->currentState.position++;
		});
		this->timer.hardwareTimer->resume();
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
		if(this->motionProfile.maximumVelocity > MOTION_MAX_VELOCITY) {
			this->motionProfile.maximumVelocity = MOTION_MAX_VELOCITY;
		}

		this->updateMotion();
	}

	//----------
	bool
	MotionControl::processIncomingByKey(const char * key, Stream& stream)
	{
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

				// VELOCITY
				if(arraySize >= 2) {
					if(!msgpack::readInt<int32_t>(stream, this->motionProfile.maximumVelocity)) {
						return false;
					}
				}

				// ACCELERATION
				if(arraySize >= 3) {
					if(!msgpack::readInt<int32_t>(stream, this->motionProfile.acceleration)) {
						return false;
					}
				}

				// MIN VELOCITY
				if(arraySize >= 4) {
					if(!msgpack::readInt<int32_t>(stream, this->motionProfile.minimumVelocity)) {
						return false;
					}
				}

				return true;
			}
		}
		return false;
	}

	//----------
	// Remarkable August 2023 p1
	void
	MotionControl::updateMotion()
	{
		// Firstly let's try a version where always moving in one direction

		// Calculate a dt value in microseconds
		auto now = micros();
		auto dt = now - this->lastTime;
		this->lastTime = now;

		// Check if we need to move or not
		if(this->targetPosition <= this->currentState.position) {
			this->motorDriver.setEnabled(false);
			this->timer.hardwareTimer->pause();
			this->timer.running = false;
			this->currentState.velocity = 0; // reset velocity for next movement
			return;
		}

		// We need to move

		// Are we accelerating or deccelerating?
		bool needsDeccelerate = false;
		if(this->currentState.velocity > this->motionProfile.minimumVelocity) {
			auto remainingSteps = this->targetPosition - this->currentState.position;

			// Calculate the time available in rest of motion profile if it's all in decceleration
			auto timeLeftInMotionProfile = (float) remainingSteps * 2.0f / (float) this->currentState.velocity;

			// Calculate time it would take to deccelerate to v=0
			auto timeItWouldTakeToDeccelerate = (float) this->currentState.velocity / (float) this->motionProfile.acceleration;

			// Decide if we should be deccelerating
			if(timeLeftInMotionProfile <= timeItWouldTakeToDeccelerate) {
				needsDeccelerate = true;
			}
		}
		micros();
		if(!needsDeccelerate) {
			// (1) - Accelerating or top speed

			if(this->currentState.velocity < this->motionProfile.maximumVelocity) {
				// Accelerate
				this->currentState.velocity += this->motionProfile.acceleration * dt / 1000000;
			}

			// Cap speed
			if(this->currentState.velocity >= this->motionProfile.maximumVelocity) {
				this->currentState.velocity = this->motionProfile.maximumVelocity;
			}
			
		}
		else {
			// (2) - Deccelerating

			if(this->currentState.velocity > this->motionProfile.minimumVelocity) {
				// Deccelerate
				this->currentState.velocity -= this->motionProfile.acceleration * dt / 1000000;
			}
		}

		// Set the velocity
		this->timer.hardwareTimer->setOverflow(this->currentState.velocity, TimerFormat_t::HERTZ_FORMAT);

		// Set 50% duty
		this->timer.hardwareTimer->setCaptureCompare(this->timer.channel
			, 127
			, TimerCompareFormat_t::RESOLUTION_8B_COMPARE_FORMAT);

		// Enable the motor driver for the movements to happen
		this->motorDriver.setEnabled(true);

		// Start the timer (if paused)
		if(!this->timer.running) {
			this->timer.hardwareTimer->resume();
			this->timer.running = true;
		}

		// Print a debug message whilst moving
		{
			char message[64];
			sprintf(message, "%d -> %d (%d)"
				, this->currentState.position
				, this->targetPosition
				, this->currentState.velocity);
			log(LogLevel::Status, message);
		}
	}
}