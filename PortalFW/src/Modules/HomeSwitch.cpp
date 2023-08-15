#include "HomeSwitch.h"
#include "Arduino.h"

void
interruptCallback()
{
	for(auto homeSwitch : Modules::HomeSwitch::allHomeSwitches) {
		homeSwitch->handleInterrupt();
	}
}

namespace Modules {
#pragma mark HomeSwitch
	//----------
	std::set<HomeSwitch*> HomeSwitch::allHomeSwitches;

	//----------
	HomeSwitch::Config
	HomeSwitch::Config::A()
	{
		Config config;
		{
			config.pinLeftSwitch = PC7;
			config.pinRightSwitch = PC13;
		}

		return config;
	}

	//----------
	HomeSwitch::Config
	HomeSwitch::Config::B()
	{
		Config config;
		{
			config.pinLeftSwitch = PC14;
			config.pinRightSwitch = PC15;
		}

		return config;
	}

#pragma mark HomeSwitch
	//----------
	HomeSwitch::HomeSwitch(const Config& config)
	: config(config)
	{
		pinMode(this->config.pinLeftSwitch, INPUT);
		pinMode(this->config.pinRightSwitch, INPUT);
		attachInterrupt(digitalPinToInterrupt(this->config.pinLeftSwitch), interruptCallback, FALLING);
		attachInterrupt(digitalPinToInterrupt(this->config.pinRightSwitch), interruptCallback, FALLING);

		HomeSwitch::allHomeSwitches.insert(this);
	}

	//----------
	const char *
	HomeSwitch::getTypeName() const
	{
		return "HomeSwitch";
	}

	//-----------
	void
	HomeSwitch::handleInterrupt()
	{
		if(digitalRead(this->config.pinLeftSwitch) == LOW) {
			digitalWrite(PB3, HIGH);
		}
		if(digitalRead(this->config.pinRightSwitch) == LOW) {
			digitalWrite(PB4, HIGH);
		}
	}

	//-----------
	bool
	HomeSwitch::getRightActive() const
	{
		return digitalRead(this->config.pinRightSwitch) == LOW;
	}

	//-----------
	bool
	HomeSwitch::getLeftActive() const
	{
		return digitalRead(this->config.pinLeftSwitch) == LOW;
	}
}