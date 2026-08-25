#pragma once
#include "pins_arduino.h"
#include "Base.h"

#include "Types.h"

// 5V @ 20 Ohms from their spec sheet
#define MOTORDRIVERSETTINGS_MAX_CURRENT 0.25f

// The current the board comes up at, before anything has had a chance to command otherwise.
// Overridable as a build flag so a bring-up build can start gentler than production does
// without editing this file -- see application_bank_optical_bringup in platformio.ini.
//
// Persisted settings override this before motor initialisation.
//
// 250 mA -- the hardware maximum, and now simply what a module runs at. 150 mA was chosen on
// bench evidence that 100-250 mA are indistinguishable for slip and for homing precision, which
// remains true; what changed is that keeping the two numbers apart bought a laddering path that
// never paid. Startup now makes one attempt at full torque and reports, rather than making
// several at rising current and reporting later. See Routines::calibrateAxisFastHome, whose
// recovery block is left in place and self-disables once the current is already at the ceiling.
#ifndef MOTORDRIVERSETTINGS_DEFAULT_CURRENT
	#define MOTORDRIVERSETTINGS_DEFAULT_CURRENT MOTORDRIVERSETTINGS_MAX_CURRENT
#endif

namespace Modules {
	class MotorDriverSettings : public Base {
	public:
		struct Config {
			uint32_t pinM0 = PB1;
			uint32_t pinM1 = PB2;
			uint32_t pinVREF = PB15;
			uint32_t pinSleep = PB0;
			float vrefRatio { 10.0f / (22.0f + 10.0f) };
			float initialCurrent = MOTORDRIVERSETTINGS_DEFAULT_CURRENT;
		};

		enum MicrostepResolution : uint8_t {
			_1 = 0,
			_2 = 1,
			_4 = 2,
			_8 = 3,
			_16 = 4,
			_32 = 5,
			_128 = 7,
			_256 = 8,
			Default = 5
		};

		typedef float Amps;

		MotorDriverSettings(const Config&);

		const char * getTypeName() const;

		void setMicrostepResolution(MicrostepResolution);
		MicrostepResolution getMicrostepResolution() const;
		Steps getMicrostepsPerStep() const;

		void setSleep(bool);
		bool getSleep() const;

		void setCurrent(Amps);
		float getCurrent() const;
	private:
		const Config config;

		void pushState();
		void pushMicrostepResoltuion();
		void pushSleep();
		void pushCurrent();

		bool processIncomingByKey(const char * key, Stream &) override;

		struct {
			MicrostepResolution microStepResolution = MicrostepResolution::Default;
			bool sleep = false;
			Amps current = MOTORDRIVERSETTINGS_DEFAULT_CURRENT;
		} state;
	};
}
