#pragma once

#include "Base.h"

#include <stdint.h>
#include <stddef.h>
#include <set>

// 8-bit PWM duty (0-255) for the comparator threshold DAC on PC15.
// Vref = 3.3V * duty/255. NEEDS BENCH CALIBRATION: raise the duty if
// home is never seen, lower it if home is always seen (false positives).
#define HOMESWITCHOPTICAL_DEFAULT_THRESHOLD 220

namespace Modules {
	class HomeSwitchOptical : public Base {
	public:
		struct Config
		{
			uint32_t pinSensor;

			static Config A();
			static Config B();
		};

		HomeSwitchOptical(const Config& = Config());
		const char * getTypeName() const;
		void setup() override;

		static std::set<HomeSwitchOptical*> allHomeSwitches;

		bool getForwardsActive() const;
		bool getBackwardsActive() const;

		// For callers that need a direct (register-level) read of the sensor in
		// an ISR, where Arduino digitalRead is too slow.
		uint32_t getPinSensor() const { return config.pinSensor; }

		// Shared comparator threshold (PC15). Static - one signal feeds both axes.
		static void setThreshold(uint8_t duty);
		static uint8_t getThreshold();
	protected:
		const Config config;
	};
}
