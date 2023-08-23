#pragma once

#include "Base.h"
#include <stdint.h>
#include <stddef.h>

#include "HardwareTimer.h"

#include "MotorDriverSettings.h"
#include "MotorDriver.h"
#include "HomeSwitch.h"

#include "Exception.h"
#include "Types.h"

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
		struct MotionProfile {
			StepsPerSecond maximumSpeed = 30000;
			StepsPerSecondPerSecond acceleration = 10000;
			StepsPerSecond minimumSpeed = 100;
		};

		struct MotionState {
			bool motorRunning = false;
			StepsPerSecond speed = 0;
			bool direction = true;
		};

		struct MeasureRoutineSettings {
			uint8_t tries = 2;
			uint8_t timeout_s = 120;
			StepsPerSecond slowMoveSpeed = 2000;
			Steps backOffDistance = MOTION_STEPS_PER_PRISM_ROTATION / 100; // Full steps
			Steps debounceDistance = 10; // Full steps
		};

		MotionControl(MotorDriverSettings&
			, MotorDriver&
			, HomeSwitch&);

		const char * getTypeName() const;
		void update();

		void testTimer();
		void initTimer();
		void deinitTimer();

		void zeroCurrentPosition();
		void setCurrentPosition(Steps steps);

		void stop();
		void run(bool direction, StepsPerSecond speed);

		void disableInterrupt();
		void enableInterrupt();

		Steps getPosition() const;

		void setTargetPosition(Steps steps);
		Steps getTargetPosition() const;

		const MotionProfile & getMotionProfile() const;
		void setMotionProfile(const MotionProfile&);
		
		bool getIsRunning() const;

		Steps getMicrostepsPerPrismRotation() const;

		// Warning : This routine loses homing
		Exception measureBacklashRoutine(const MeasureRoutineSettings&);
		Exception homeRoutine(const MeasureRoutineSettings&);

		void reportStatus(msgpack::Serializer&) override;
	protected:
		bool processIncomingByKey(const char * key, Stream &) override;

		void updateStepCount();
		void updateMotion();

		MotionState calculateMotionState(unsigned long dt_us) const;

		MotorDriverSettings& motorDriverSettings;
		MotorDriver& motorDriver;
		HomeSwitch& homeSwitch;

		struct {
			HardwareTimer* hardwareTimer = nullptr;
			uint32_t channel;
			bool running = false;
		} timer;

		MotionProfile motionProfile;

		struct {
			Steps systemBacklash = 1499;
			Steps positionWithinBacklash = 0; // negative when going forwards
		} backlashControl;

		uint32_t lastTime = 0;
		Steps targetPosition = 0;

		// Count steps happening in interrupt
		Steps stepsInInterrupt = 0;
		uint32_t lastStepDetectedOrRunStart = 0;
		const uint32_t maxTimeWithoutSteps = 1000;
		uint8_t stepWatchdogResetCount = 0;
		const uint32_t maxStepWatchdogResets = 16;

		bool interruptEnabed = false;
		Steps position = 0;
		MotionState currentMotionState;
	};
}
