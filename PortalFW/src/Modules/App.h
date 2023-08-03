#pragma once

#include "Log.h"
#include "MotorDriverSettings.h"
#include "MotorDriver.h"
#include "ID.h"
#include "RS485.h"
#include "HomeSwitch.h"

#include <memory>
#include <vector>

namespace Modules {
	class App : public Base {
	public:
		void setup();
		void update();
		
		ID * id;
		RS485 * rs485;

		MotorDriverSettings * motorDriverSettings;
		MotorDriver * motorDriverA;
		MotorDriver * motorDriverB;

		HomeSwitch * homeSwitchA;
		HomeSwitch * homeSwitchB;
	};
}