#pragma once

#include "Base.h"
#include <stdint.h>
#include <stddef.h>

#include "HardwareTimer.h"

#include "MotorDriver.h"
#include "HomeSwitch.h"

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
		
		MotorDriver& motorDriver;
		HomeSwitch& homeSwitch;

		struct {
			StepsPerSecond minimumVelocity = 1000;
			StepsPerSecond maximumVelocity = 100000;
			StepsPerSecondPerSecond maximumAcceleration = 1000;
		} motionProfile;

		struct {
			StepsPerSecond position;
			
		} motionState;
	};
}
