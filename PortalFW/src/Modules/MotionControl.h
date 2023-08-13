#pragma once

#include "Base.h"
#include <stdint.h>
#include <stddef.h>

#include "HardwareTimer.h"

#include "MotorDriver.h"
#include "HomeSwitch.h"

// Above this the device locks up because interrupts are too rapid
#define MOTION_MAX_SPEED 80000

namespace Modules {
	class MotionControl : public Base {
	public:
		typedef int32_t Steps;
		typedef int32_t StepsPerSecond;
		typedef int32_t StepsPerSecondPerSecond;

		struct MotionState {
			bool motorRunning = false;
			StepsPerSecond speed = 0;
			bool direction = true;
		};

		MotionControl(MotorDriver&, HomeSwitch&);

		const char * getTypeName() const;
		void update();

		void setTargetPosition(Steps steps);
		void setMaximumVelocty(StepsPerSecond stepsPerSecond);
		void setMaximumAcceleration(StepsPerSecondPerSecond stepsPerSecondPerSecond);

		void stop();
	protected:
		bool processIncomingByKey(const char * key, Stream &) override;
		void updateMotion();
		MotionState calculateMotionState(unsigned long dt_us) const;

		MotorDriver& motorDriver;
		HomeSwitch& homeSwitch;

		struct {
			HardwareTimer* hardwareTimer;
			uint32_t channel;
			bool running = false;
		} timer;

		struct {
			StepsPerSecond maximumSpeed = 100000;
			StepsPerSecondPerSecond acceleration = 500;
			StepsPerSecond minimumSpeed = 1000;

			int64_t positionEpsilon = 1;
		} motionProfile;

		uint32_t lastTime = 0;
		Steps targetPosition = 0;

		Steps position = 0;
		MotionState currentMotionState;
	};
}
