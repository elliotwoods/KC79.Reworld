#pragma once
#include "pins_arduino.h"
#include "Base.h"

#include "Types.h"

// 5V @ 20 Ohms from their spec sheet
#define MOTORDRIVERSETTINGS_MAX_CURRENT 0.25f
#define MOTORDRIVERSETTINGS_DEFAULT_CURRENT MOTORDRIVERSETTINGS_MAX_CURRENT

namespace Modules {
	class MotorDriverSettings : public Base {
	public:
		struct Config {
			uint32_t pinM0 = PB1;
			uint32_t pinM1 = PB2;
			uint32_t pinVREF = PB15;
			uint32_t pinSleep = PB0;
			float vrefRatio { 10.0f / (22.0f + 10.0f) };
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
			Amps current = 0.1f;
		} state;
	};
}
