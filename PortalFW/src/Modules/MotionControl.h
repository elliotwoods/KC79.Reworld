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
	class Routines;
	
	class MotionControl : public Base {
	public:
		struct MotionProfile {
			StepsPerSecond maximumSpeed = 7040 * 4; // 4 * note A8
			StepsPerSecondPerSecond acceleration = 10000;
			StepsPerSecond minimumSpeed = 5;
		};

		struct MotionState {
			bool motorRunning = false;
			StepsPerSecond speed = 0;
			bool direction = true;
		};

		struct MeasureRoutineSettings {
			// Note that not all settings are used in all routines
			uint8_t timeout_s = 120;
			StepsPerSecond slowMoveSpeed = 2000;
			Steps backOffDistance = MOTION_STEPS_PER_PRISM_ROTATION / 100; // Full steps
			Steps debounceDistance = 10; // Full steps
			uint8_t tryCount = 3;
		};

		struct SwitchSeen {
			bool seen = false;
			Steps positionFirstSeen = 0;
		};

		struct SwitchesSeen {
			SwitchSeen forwards;
			SwitchSeen backwards;
		};

		MotionControl(MotorDriverSettings&
			, MotorDriver&
			, HomeSwitch&);

		static bool readMeasureRoutineSettings(Stream&, MeasureRoutineSettings&);
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

		void attachCustomInterrupt(const std::function<void()> &);
		void disableCustomInterrupt();

		void attachSwitchesSeenInterrupt(SwitchesSeen &);
		void disableSwitchesSeenInterrupt();

		Steps getPosition() const;

		void setTargetPosition(Steps steps);
		Steps getTargetPosition() const;

		void setTargetPositionWithMotionFiltering(Steps);

		const MotionProfile & getMotionProfile() const;
		void setMotionProfile(const MotionProfile&);
		
		bool getIsRunning() const;

		Steps getMicrostepsPerPrismRotation() const;

		// Warning : This routine loses homing
		Exception unjamRoutine(const MeasureRoutineSettings&);
		Exception tuneCurrentRoutine(const MeasureRoutineSettings&);
		Exception measureBacklashRoutine(const MeasureRoutineSettings&);
		Exception homeRoutine(const MeasureRoutineSettings&);

		void reportStatus(msgpack::Serializer&) override;

		bool isBacklashCalibrated() const;
		bool isHomeCalibrated() const;
	protected:
		friend Routines;
		bool processIncomingByKey(const char * key, Stream &) override;

		void updateStepCount();
		void updateFilteredMotion();
		void updateMotion();

		MotionState calculateMotionState(unsigned long dt_us) const;

		void homeWhilstRunningForwards(Steps position);
		void homeWhilstRunningBackwards(Steps position);

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
			bool backlashCalibrated = false;
			Steps positionWithinBacklash = 0; // negative when going forwards
		} backlashControl;

		struct {
			bool liveHomingEnabled = false;
			Steps switchSize = 0; // size between forwards and backwards start engagement
		} homing;

		// This is used to smooth out motion between packets
		struct {
			bool enabled = true;
			uint32_t lastMoveMessageTime = 0;
			const uint32_t allowedDuration = 2000;

			bool initialised = false;
			Steps velocity;
			Steps lastPosition;

			bool active = false;
		} motionFiltering;

		bool homeCalibrated = false;

		uint32_t lastTime = 0;
		Steps targetPosition = 0;

		// Count steps happening in interrupt
		struct {
			Steps steps = 0;
			SwitchesSeen switchesSeen;
		} inInterrupt;

		bool interruptEnabed = false;
		Steps position = 0;
		MotionState currentMotionState;
	};
}
