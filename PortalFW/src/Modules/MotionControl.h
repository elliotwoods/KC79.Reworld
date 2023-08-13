#pragma once

#include "Base.h"
#include <stdint.h>
#include <stddef.h>

#include "HardwareTimer.h"

#include "MotorDriver.h"
#include "HomeSwitch.h"

// Above this the device locks up because interrupts are too rapid
#define MOTION_MAX_VELOCITY 80000

namespace Modules {
	class MotionControl : public Base {
	public:
		typedef int32_t Steps;
		typedef int32_t StepsPerSecond;
		typedef int32_t StepsPerSecondPerSecond;

		MotionControl(MotorDriver&, HomeSwitch&);

		const char * getTypeName() const;
		void update();

		void setTargetPosition(Steps steps);
		void setMaximumVelocty(StepsPerSecond stepsPerSecond);
		void setMaximumAcceleration(StepsPerSecondPerSecond stepsPerSecondPerSecond);
	protected:
		bool processIncomingByKey(const char * key, Stream &) override;
		void updateMotion();

		MotorDriver& motorDriver;
		HomeSwitch& homeSwitch;

		struct {
			HardwareTimer* hardwareTimer;
			uint32_t channel;
			bool running = false;
		} timer;

		struct {
			StepsPerSecond maximumVelocity = 100000;
			StepsPerSecondPerSecond acceleration = 500;
			StepsPerSecond minimumVelocity = 1000;
		} motionProfile;

		uint32_t lastTime = 0;
		Steps targetPosition = 0;

		struct {
			Steps position = 0;
			StepsPerSecond velocity = 0;
		} currentState;
	};
}
