#pragma once

#ifndef GUI_DISABLED
	#include "GUI.h"
#endif

#include "Logger.h"
#include "MotorDriverSettings.h"
#include "MotorDriver.h"
#include "ID.h"
#include "RS485.h"
#include "HomeSwitch.h"
#include "MotionControl.h"

#include <memory>
#include <vector>

#define STARTUP_ENABLED false

namespace Modules {
	class App : public Base {
	public:
		const char * getTypeName() const;

		void setup();
		void update();
		void reportStatus(msgpack::Serializer&);
		void initRoutine(uint8_t tryCount);
		
#ifndef GUI_DISABLED
		GUI * gui;
#endif
		ID * id;
		RS485 * rs485;

		MotorDriverSettings * motorDriverSettings;
		MotorDriver * motorDriverA;
		MotorDriver * motorDriverB;

		HomeSwitch * homeSwitchA;
		HomeSwitch * homeSwitchB;

		MotionControl * motionControlA;
		MotionControl * motionControlB;
	protected:
		bool processIncomingByKey(const char * key, Stream &) override;
	};
}