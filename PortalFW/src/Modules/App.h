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
#include "KeyframeMotionControl.h"
#include "../PersistentStorage.h"

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

		// Whether an escape has been requested and not yet consumed. Routines that retry a
		// sub-routine in a loop must check this between attempts -- otherwise an escape only
		// aborts the attempt in flight and the loop immediately starts another one, so the
		// operator has to send escape once per remaining retry to actually stop. The flag is
		// cleared at the top of App::update(), which cannot run while a routine is blocking,
		// so it stays readable for the whole of a routine chain.
		static bool getShouldEscapeFromRoutine();

		MotionControl * getMotionControl(uint8_t);
		uint32_t getProvisionSerial() const;
		uint16_t getOperatingCurrentMa() const;
		bool getFullCurrentHomeRecovery() const;
		bool persistOperatingSettings(uint16_t currentMa, bool recovery);
		bool persistOpticalCalibration(MotionControl * axis);

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

		KeyframeMotionControl * keyframeMotionControl;
		
	protected:
		static App * instance;
		bool processIncomingByKey(const char * key, Stream &) override;
		bool isInsideRoutine = true;
		bool shouldEscapeFromRoutine = false;
		PersistentStorage::Identity persistentIdentity;
		PersistentStorage::Settings persistentSettings;
	};
}
