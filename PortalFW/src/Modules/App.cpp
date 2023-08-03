#include "App.h"
#include <Arduino.h>

#include "GUI/Controller.h"
#include "GUI/Panels/SplashScreen.h"

namespace Modules {
	//----------
	void
	App::setup()
	{
		Logger::setup();
		GUI::Controller::X().init(std::make_shared<GUI::Panels::SplashScreen>());

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
		GUI::Controller::X().update();
		this->id->update();
		this->rs485->update();
		this->motorDriverSettings->update();
		this->motorDriverA->update();
		this->motorDriverB->update();
		this->homeSwitchA->update();
		this->homeSwitchB->update();
	}
}