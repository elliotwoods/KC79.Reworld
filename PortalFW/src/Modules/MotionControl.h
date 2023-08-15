#pragma once

#include "Base.h"
#include <stdint.h>
#include <stddef.h>

#include "HardwareTimer.h"

#include "MotorDriverSettings.h"
#include "MotorDriver.h"
#include "HomeSwitch.h"

#include "Exception.h"

// Above this the device locks up because interrupts are too rapid
#define MOTION_MAX_SPEED 80000

#define MOTION_STEPS_PER_MOTOR_ROTATION 32
#define MOTION_STEPPER_GEAR_REDUCTION 9759 / 296
#define MOTION_GEAR_DRIVE 21
#define MOTION_GEAR_RING 118

#define MOTION_STEPS_PER_PRISM_ROTATION ( MOTION_STEPS_PER_MOTOR_ROTATION \
	* MOTION_GEAR_RING \
	* MOTION_STEPPER_GEAR_REDUCTION \
	/ MOTION_GEAR_DRIVE )

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

		struct BacklashMeasureSettings {
			StepsPerSecond fastMoveSpeed = 80000;
			StepsPerSecond slowMoveSpeed = 1000;
			Steps backOffDistance = MOTION_STEPS_PER_PRISM_ROTATION / 100; // Full steps
			Steps debounceDistance = 2; // Full steps
		};

		MotionControl(MotorDriverSettings&
			, MotorDriver&
			, HomeSwitch&);

		const char * getTypeName() const;
		void update();

		void zeroCurrentPosition();
		void setCurrentPosition(Steps steps);
		void setTargetPosition(Steps steps);
		void setMaximumVelocty(StepsPerSecond stepsPerSecond);
		void setMaximumAcceleration(StepsPerSecondPerSecond stepsPerSecondPerSecond);

		void stop();
		void run(bool direction, StepsPerSecond speed);

		void disableInterrupt();
		void enableInterrupt();
	protected:
		bool processIncomingByKey(const char * key, Stream &) override;
		void updateMotion();
		MotionState calculateMotionState(unsigned long dt_us) const;
		
		// Warning : This routine ignores the step counting, therefore loses homing
		Exception measureBacklashRoutine(uint8_t timeout_s, const BacklashMeasureSettings&);

		MotorDriverSettings& motorDriverSettings;
		MotorDriver& motorDriver;
		HomeSwitch& homeSwitch;

		struct {
			HardwareTimer* hardwareTimer;
			uint32_t channel;
			bool running = false;
		} timer;

		struct {
			StepsPerSecond maximumSpeed = 60000;
			StepsPerSecondPerSecond acceleration = 10000;
			StepsPerSecond minimumSpeed = 1000;
		} motionProfile;

		struct {
			Steps systemBacklash = 1612;
			Steps positionWithinBacklash = 0; // negative when going forwards
		} backlashControl;

		uint32_t lastTime = 0;
		Steps targetPosition = 0;

		bool interruptEnabed = false;
		Steps position = 0;
		MotionState currentMotionState;
	};
}
