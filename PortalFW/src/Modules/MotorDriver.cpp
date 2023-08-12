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
		}

		return false;
	}
}