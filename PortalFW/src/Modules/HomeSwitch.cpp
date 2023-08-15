#include "HomeSwitch.h"
#include "Arduino.h"

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
			config.pinBackwardsSwitch = PC7;
			config.pinForwardsSwitch = PC13;
		}

		return config;
	}

	//----------
	HomeSwitch::Config
	HomeSwitch::Config::B()
	{
		Config config;
		{
			config.pinBackwardsSwitch = PC14;
			config.pinForwardsSwitch = PC15;
		}

		return config;
	}

#pragma mark HomeSwitch
	//----------
	HomeSwitch::HomeSwitch(const Config& config)
	: config(config)
	{
		pinMode(this->config.pinBackwardsSwitch, INPUT);
		pinMode(this->config.pinForwardsSwitch, INPUT);

		HomeSwitch::allHomeSwitches.insert(this);
	}

	//----------
	const char *
	HomeSwitch::getTypeName() const
	{
		return "HomeSwitch";
	}

	//-----------
	bool
	HomeSwitch::getForwardsActive() const
	{
		return digitalRead(this->config.pinForwardsSwitch) == LOW;
	}

	//-----------
	bool
	HomeSwitch::getBackwardsActive() const
	{
		return digitalRead(this->config.pinBackwardsSwitch) == LOW;
	}
}