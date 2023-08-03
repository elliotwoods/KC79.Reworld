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
}