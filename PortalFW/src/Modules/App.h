#pragma once

#include "GUI.h"
#include "Logger.h"
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
		const char * getTypeName() const;

		void setup();
		void update();
		
		GUI * gui;
		ID * id;
		RS485 * rs485;

		MotorDriverSettings * motorDriverSettings;
		MotorDriver * motorDriverA;
		MotorDriver * motorDriverB;

		HomeSwitch * homeSwitchA;
		HomeSwitch * homeSwitchB;
	protected:
		bool processIncomingByKey(const char * key, Stream &) override;
	};
}