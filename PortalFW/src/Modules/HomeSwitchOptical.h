#pragma once

#include "Base.h"
#include "Arduino.h"

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

		// The comparator, read as one masked load from GPIOx->IDR.
		//
		// This is what the step ISR uses. Arduino digitalRead costs ~25-40 cycles here: it
		// resolves the pin through digitalPinToPinName's bounds-checked flash table and then
		// through get_GPIO_Port's, and at FLASH_LATENCY_2 those two lookups dominate the single
		// bit it returns. The port and mask never change after setup, so they are resolved once
		// and the ISR pays ~3 cycles. HomeSwitchTest/src/BenchMotion.cpp:49-55 has done exactly
		// this on the bench rig since the optical work started, with the same reasoning; this
		// brings it into the firmware.
		//
		// Active-high: the comparator output is push-pull and reads HIGH on the flag.
		bool getRawActive() const {
			return (this->sensorPort->IDR & this->sensorPinMask) != 0;
		}

		// Both latch inputs from one read.
		//
		// The optical switch is ONE sensor: forwards and backwards are the same pin and always
		// agree. The step ISR latches them separately, so asking for them separately made it do
		// the identical expensive read twice for one bit. This is the shape the ISR wants, and
		// the mechanical switch -- which genuinely has two pins -- answers it honestly.
		struct RawState {
			bool forwards;
			bool backwards;
		};
		RawState getRawState() const {
			const bool active = this->getRawActive();
			return RawState { active, active };
		}

		// For callers that need a direct (register-level) read of the sensor in
		// an ISR, where Arduino digitalRead is too slow.
		uint32_t getPinSensor() const { return config.pinSensor; }

		// Shared comparator threshold (PC15). Static - one signal feeds both axes.
		static void setThreshold(uint8_t duty);
		static uint8_t getThreshold();
	protected:
		const Config config;

		// Resolved once in the constructor; see getRawActive().
		GPIO_TypeDef * sensorPort = nullptr;
		uint32_t sensorPinMask = 0;
	};
}
