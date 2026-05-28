#include "HomeSwitchMechanical.h"
#include "Arduino.h"

namespace Modules {
#pragma mark HomeSwitchMechanical
	//----------
	std::set<HomeSwitchMechanical*> HomeSwitchMechanical::allHomeSwitches;

	//----------
	HomeSwitchMechanical::Config
	HomeSwitchMechanical::Config::A()
	{
		Config config;
		{
			config.pinBackwardsSwitch = PC7;
			config.pinForwardsSwitch = PC13;
		}

		return config;
	}

	//----------
	HomeSwitchMechanical::Config
	HomeSwitchMechanical::Config::B()
	{
		Config config;
		{
			config.pinBackwardsSwitch = PC14;
			config.pinForwardsSwitch = PC15;
		}

		return config;
	}

#pragma mark HomeSwitchMechanical
	//----------
	HomeSwitchMechanical::HomeSwitchMechanical(const Config& config)
	: config(config)
	{
		pinMode(this->config.pinBackwardsSwitch, INPUT);
		pinMode(this->config.pinForwardsSwitch, INPUT);

		HomeSwitchMechanical::allHomeSwitches.insert(this);
	}

	//----------
	const char *
	HomeSwitchMechanical::getTypeName() const
	{
		return "HomeSwitchMechanical";
	}

	//-----------
	bool
	HomeSwitchMechanical::getForwardsActive() const
	{
		return digitalRead(this->config.pinForwardsSwitch) == LOW;
	}

	//-----------
	bool
	HomeSwitchMechanical::getBackwardsActive() const
	{
		return digitalRead(this->config.pinBackwardsSwitch) == LOW;
	}
}
