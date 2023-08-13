#include "App.h"
#include <Arduino.h>

namespace Modules {
	//----------
	const char *
	App::getTypeName() const
	{
		return "App";
	}

	//----------
	void
	App::setup()
	{
		Logger::setup();

#ifndef GUI_DISABLED
		this->gui = new GUI();
		this->gui->setup();
#endif

		this->id = new ID();
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

#ifndef GUI_DISABLED
		this->gui->update();
#endif
		// this->motorDriverSettings->setCurrent(0.15f);
		// this->motorDriverA->testRoutine();
		// this->motorDriverB->testRoutine();
		// this->motorDriverSettings->setCurrent(0.05f);
	}

	//----------
	bool
	App::processIncomingByKey(const char * key, Stream & stream)
	{
		if(strcmp(key, "id") == 0) {
			return this->id->processIncoming(stream);
		}
		
		if(strcmp(key, "motorDriverSettings") == 0) {
			return this->motorDriverSettings->processIncoming(stream);
		}

		if(strcmp(key, "motorDriverA") == 0) {
			return this->motorDriverA->processIncoming(stream);
		}
		if(strcmp(key, "motorDriverB") == 0) {
			return this->motorDriverB->processIncoming(stream);
		}

		return false;
	}
}