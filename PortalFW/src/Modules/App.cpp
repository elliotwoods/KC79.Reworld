#include "App.h"
#include <Arduino.h>

namespace Modules {
	//----------
	void
	App::setup()
	{
		Logger::setup();

		this->gui = new GUI();
		this->gui->setup();

		this->id = new ID(ID::Config());
		this->id->setup();

		this->rs485 = new RS485(this);
		this->rs485->setup();
		
		this->motorDriverSettings = new MotorDriverSettings(MotorDriverSettings::Config());
		this->motorDriverSettings->setup();

		this->motorDriverA = new MotorDriver(MotorDriver::Config::MotorA());
		this->motorDriverA->setup();

		this->motorDriverB = new MotorDriver(MotorDriver::Config::MotorB());
		this->motorDriverB->setup();

		this->homeSwitchA = new HomeSwitch(HomeSwitch::Config::A());
		this->homeSwitchA->setup();

		this->homeSwitchB = new HomeSwitch(HomeSwitch::Config::B());
		this->homeSwitchB->setup();
	}

	//----------
	void
	App::update()
	{
		this->id->update();
		this->rs485->update();
		this->motorDriverSettings->update();
		this->motorDriverA->update();
		this->motorDriverB->update();
		this->homeSwitchA->update();
		this->homeSwitchB->update();

		this->gui->update();
	}
}