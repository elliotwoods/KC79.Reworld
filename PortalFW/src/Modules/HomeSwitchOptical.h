#pragma once

#include "Base.h"

#include <stdint.h>
#include <stddef.h>
#include <set>

// 8-bit PWM duty (0-255) for the comparator threshold DAC on PC15.
// Vref = 3.3V * duty/255. Crossing duty is INVERSE to reflectance: the switch reads active when
// the surface's crossing is BELOW the threshold, so a lower crossing means a brighter surface.
//
// This is the power-on value, and it is what every routine that does not set its own threshold
// runs at -- measureCycleRoutine in particular, which happens before homing has calibrated
// anything. It must therefore be the same operating point homing uses, not an independent
// guess: it was 220, which census measurements on a silver-painted production module put only
// ten counts above axis A's detection floor of 210, where the flag is 53 microsteps wide
// instead of ~280. Keep this equal to FASTHOME_T_DEFAULT (MotionControl.cpp), where the
// measurement that chose the number is recorded in full.
#define HOMESWITCHOPTICAL_DEFAULT_THRESHOLD 235

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
