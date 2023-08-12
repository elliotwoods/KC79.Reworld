#include "MotorDriver.h"
#include "Logger.h"

#include <Arduino.h>

namespace Modules {
#pragma mark MotorDriver::Config
	//----------
	MotorDriver::Config
	MotorDriver::Config::MotorA()
	{
		Config config;
		{
			config.Fault = PA4;
			config.Enable = PA5;
			config.Step = PA6;
			config.Direction = PA7;
		}
		return config;
	}

	//----------
	MotorDriver::Config
	MotorDriver::Config::MotorB()
	{
		Config config;
		{
			config.Fault = PA8;
			config.Enable = PA9;
			config.Step = PA10;
			config.Direction = PA11;
		}
		return config;
	}

#pragma mark MotorDriver
	//----------
	MotorDriver::MotorDriver(const Config& config)
	: config(config)
	{
		pinMode(this->config.Fault, INPUT);
		pinMode(this->config.Enable, OUTPUT);
		pinMode(this->config.Step, OUTPUT);
		pinMode(this->config.Direction, OUTPUT);

		this->pushState();
	}

	//----------
	const char *
	MotorDriver::getTypeName() const
	{
		return "MotorDriver";
	}

	//----------
	void
	MotorDriver::setEnabled(bool value)
	{
		this->state.enabled = value;
		this->pushEnabled();
	}

	//----------
	bool
	MotorDriver::getEnabled() const
	{
		return this->state.enabled;
	}

	//----------
	void
	MotorDriver::setDirection(bool value)
	{
		this->state.direction = value;
		this->pushDirection();
	}

	//----------
	bool
	MotorDriver::getDirection() const
	{
		return this->state.direction;
	}

	//----------
	void
	MotorDriver::testSteps(size_t stepCount, uint32_t delayBetweenSteps)
	{
		for(size_t i=0; i<stepCount; i++) {
			digitalWrite(this->config.Step, HIGH);
			digitalWrite(this->config.Step, LOW);
			delayMicroseconds(delayBetweenSteps);
		}
	}

	//----------
	void
	MotorDriver::testRoutine()
	{
		this->setEnabled(true);
		
		int slowest = 50;
		int count = 100000;

		log(LogLevel::Status, "Test routine CCW");
		{
			this->setDirection(false);
			int i=slowest;
			for(; i>1; i--) {
				this->testSteps(count/i, i);
			}
			for(; i<slowest; i++) {
				this->testSteps(count/i, i);
			}
		}

		delay(100);

		log(LogLevel::Status, "Test routine CW");
		{
			this->setDirection(true);
			int i=slowest;
			for(; i>1; i--) {
				this->testSteps(count/i, i);
			}
			for(; i<slowest; i++) {
				this->testSteps(count/i, i);
			}
		}

		delay(100);

		log(LogLevel::Status, "Test routine complete");

		this->setEnabled(false);
	}

	//----------
	void
	MotorDriver::testTimer(uint32_t period_us, uint32_t target_count)
	{
		this->setEnabled(true);

		// note the library accepts all different formats (ticks, us, hz)

		auto stepPin = digitalPinToPinName(this->config.Step);
		this->timer.timer = (TIM_TypeDef *) pinmap_peripheral(stepPin, PinMap_TIM);
		this->timer.hardwareTimer = new HardwareTimer(this->timer.timer);
		auto channel = STM_PIN_CHANNEL(pinmap_function(stepPin, PinMap_TIM));
		this->timer.hardwareTimer->setMode(channel, TIMER_OUTPUT_COMPARE_PWM1, stepPin);
		this->timer.hardwareTimer->setOverflow(period_us, MICROSEC_FORMAT);
		this->timer.hardwareTimer->setCaptureCompare(channel
			, 127
			, TimerCompareFormat_t::RESOLUTION_8B_COMPARE_FORMAT);
		this->timer.currentCount = 0;
		this->timer.hardwareTimer->attachInterrupt([this]() {
			this->timer.currentCount++;
		});
		this->timer.hardwareTimer->resume();

		log(LogLevel::Status, "Test begin");

		do {
			HAL_Delay(1);
			{
				char message[100];
				sprintf(message, "%d\n", (int) this->timer.currentCount);
				log(LogLevel::Status, message);
			}
		} while (this->timer.currentCount < target_count);

		log(LogLevel::Status, "Test end");
		this->timer.hardwareTimer->pause();

		// destroy it for now (so that we can call multiple times)
		delete this->timer.hardwareTimer;

		this->setEnabled(false);
	}

	//----------
	void 
	MotorDriver::pushState()
	{
		this->pushEnabled();
		this->pushDirection();
	}

	//----------
	void
	MotorDriver::pushEnabled()
	{
		digitalWrite(this->config.Enable, this->state.enabled);
	}

	//----------
	void
	MotorDriver::pushDirection()
	{
		digitalWrite(this->config.Direction, this->state.direction);
	}

	//----------
	bool
	MotorDriver::processIncomingByKey(const char * key, Stream& stream)
	{
		if(strcmp(key, "testRoutine") == 0) {
			// Next item will be a Nil
			if(!msgpack::readNil(stream, true)) {
				return false;
			}

			this->testRoutine();

			return true;
		}
		else if(strcmp(key, "testTimer") == 0) {
			// Expecting an array with 2 values [period_us, target_count]
			size_t arraySize;
			if(!msgpack::readArraySize(stream, arraySize)) {
				return false;
			}

			if(arraySize < 2) {
				return false;
			}

			uint32_t period_us;
			uint32_t target_count;
			if(!msgpack::readInt<uint32_t>(stream, period_us)) {
				return false;
			}
			if(!msgpack::readInt<uint32_t>(stream, target_count)) {
				return false;
			}

			this->testTimer(period_us, target_count);
			return true;
		}

		return false;
	}
}