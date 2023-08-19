#pragma once

#include "Base.h"
#include <stdint.h>
#include <stddef.h>

#include "HardwareTimer.h"

namespace Modules {
	class MotorDriver : public Base {
	public:
		struct Config {
			char AxisLabel;
			uint32_t Fault;
			uint32_t Enable;
			uint32_t Step;
			PinName StepTimerPin;
			uint32_t Direction;

			static Config MotorA();
			static Config MotorB();
		};

		MotorDriver(const Config&);

		const char * getTypeName() const;

		const Config& getConfig() const;

		void setEnabled(bool);
		bool getEnabled() const;

		void setDirection(bool);
		bool getDirection() const;
		
		// This is used in test and measurement routines only
		void step(uint16_t stepHalfCycleTime_us = 1) const;

		void testSteps(size_t stepCount, uint32_t delayBetweenSteps);
		void testRoutine();
		void testTimer(uint32_t period_us, uint32_t target_count);

	protected:
		void pushState();
		void pushEnabled();
		void pushDirection();

		bool processIncomingByKey(const char * key, Stream &) override;
		
		const Config config;

		struct {
			bool enabled = false;
			bool direction = false;
		} state;

		struct {
			TIM_TypeDef * timer;
			HardwareTimer * hardwareTimer;
			int32_t currentCount;
		} timer;
	};
}
