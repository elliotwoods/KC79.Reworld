#pragma once

#ifndef GUI_DISABLED
	#include "GUI.h"
#endif

#include "Logger.h"
#include "MotorDriverSettings.h"
#include "MotorDriver.h"
#include "ID.h"
#include "RS485.h"
#include "LEDs.h"
#include "HomeSwitch.h"
#include "MotionControl.h"
#include "Routines.h"

#include <memory>
#include <vector>

namespace Modules {
	class App : public Base {
	public:
		App();
		static App & X();
		const char * getTypeName() const;

		void setup();
		void update();
		void reportStatus(msgpack::Serializer&);
		
		// Use this update if you're doing a routine that's blocking the mainloop
		// e.g. to send a reboot / FW announce
		// Returns if should escape
		// This doesn't do anything with motors, switches
		static bool updateFromRoutine();
		void escapeFromRoutine();

#ifndef GUI_DISABLED
		GUI * gui;
#endif
		ID * id;
		RS485 * rs485;
		LEDs * leds;

		MotorDriverSettings * motorDriverSettings;
		MotorDriver * motorDriverA;
		MotorDriver * motorDriverB;

		HomeSwitch * homeSwitchA;
		HomeSwitch * homeSwitchB;

		MotionControl * motionControlA;
		MotionControl * motionControlB;

		Routines * routines;
		
	protected:
		static App * instance;
		bool processIncomingByKey(const char * key, Stream &) override;
		bool isInsideRoutine = true;
		bool shouldEscapeFromRoutine = false;
	};
}