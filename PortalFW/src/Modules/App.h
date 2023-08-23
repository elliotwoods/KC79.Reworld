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

namespace Modules {
	class App : public Base {
	public:
		App();
		const char * getTypeName() const;

		void setup();
		void update();
		void reportStatus(msgpack::Serializer&);
		bool initRoutine(uint8_t tryCount);
		bool calibrateRoutine(uint8_t tryCount);
		void flashLEDsRoutine(uint16_t period, uint16_t count);

		static void updateFromRoutine();

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

		// Use this update if you're doing a routine that's blocking the mainloop
		// e.g. to send a reboot / FW announce
	protected:
		static App * instance;

		bool processIncomingByKey(const char * key, Stream &) override;
		Exception walkBackAndForthRoutine(const MotionControl::MeasureRoutineSettings&);

		bool calibrated = false;
	};
}