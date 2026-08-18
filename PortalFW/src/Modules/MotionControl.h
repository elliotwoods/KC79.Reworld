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

// N * musical note A8
#define MOTION_DEFAULT_SPEED 7040 * 2

#define MOTION_STEPS_PER_MOTOR_ROTATION 32
#define MOTION_STEPPER_GEAR_REDUCTION 9759 / 296
#define MOTION_GEAR_DRIVE 21
#define MOTION_GEAR_RING 118

#define MOTION_CLEAR_SWITCH_STEPS (20000 / 128)
#define MOTION_CLEAR_BACKLASH_STEPS (30000 / 128)

#define MOTION_STEPS_PER_PRISM_ROTATION ( MOTION_STEPS_PER_MOTOR_ROTATION \
	* MOTION_GEAR_RING \
	* MOTION_STEPPER_GEAR_REDUCTION \
	/ MOTION_GEAR_DRIVE )

#define MOTION_ALLOWED_PRISM_ROTATION_ERROR 10

namespace Modules {
	class Routines;
#ifndef HOME_SWITCH_LEGACY
	struct FastHomeParams; // defined in MotionControl.cpp, alongside fastHomeRoutine
#endif

	class MotionControl : public Base {
	public:
		struct MotionProfile {
			StepsPerSecond maximumSpeed = MOTION_DEFAULT_SPEED;
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
			Steps debounceDistance = 32; // Full steps
			uint8_t tryCount = 3;
			bool stopAllRoutinesIfOneFails = false;
		};

		struct FrameSwitchEvents {
			struct Switch {
				bool seen = false;
				Steps positionSeen;
			};
			Switch forwards;
			Switch backwards;
		};

		struct SwitchesMask {
			bool forwards = false;
			bool backwards = false;
		};

		struct RoutineMoveResult {
			Exception exception = Exception::None();
			FrameSwitchEvents frameSwitchEvents;
		};

		struct HealthStatus {
			bool measureCycleOK = false;
			bool switchesOK = false;
			bool backlashOK = false;
			bool homeOK = false;

			bool allOK() const;
		};

		MotionControl(MotorDriverSettings&
			, MotorDriver&
			, HomeSwitch&);

		static bool readMeasureRoutineSettings(Stream&, MeasureRoutineSettings&);
		const char * getTypeName() const override;
		const char * getName() const override;

		void update();
		const FrameSwitchEvents & getFrameSwitchEvents() const;

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

		Steps getClosestHomePosition() const;

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
		Exception measureCycleRoutine(const MeasureRoutineSettings&);
#ifndef HOME_SWITCH_LEGACY
		// Self-calibrating optical home: replaces measureBacklashRoutine + homeRoutine for
		// the optical switch in one pass. See HomeSwitchTest/portalfw_port/PORTING.md and
		// HomeSwitchTest/reports/newring/HOME_ROUTINE_DESIGN.md for the bench provenance.
		Exception fastHomeRoutine(const MeasureRoutineSettings&);
#endif

		void reportStatus(msgpack::Serializer&) override;

		const HealthStatus & getHealthStatus() const;

		RoutineMoveResult routineMoveTo(Steps targetPosition
			, uint32_t timeout);

		// Move using standard motion profile to a position and stop when a switch is seen
		RoutineMoveResult routineMoveToUntilSeeSwitch(Steps targetPosition
			, SwitchesMask
			, uint32_t timeout);

		RoutineMoveResult routineMoveToFindSwitch(bool direction
			, StepsPerSecond speed
			, SwitchesMask
			, uint32_t timeout);

		RoutineMoveResult routineFindSwitchAccurate(bool direction
			, StepsPerSecond slowSpeed
			, bool guessPosition
			, uint32_t timeout);
		
	protected:
		friend Routines;
		bool processIncomingByKey(const char * key, Stream &) override;

		void updateStepsAndSwitches();
		void updateFilteredMotion();
		void updateMotion();

		MotionState calculateMotionState(unsigned long dt_us) const;

		// Call this function when you want to update the home offset
		// It should round your observation to the closest whole rotation (i.e. doesn't cause a zeroing)
		void homeSwitchSeenAt(Steps position, bool isForwards);

		void homeWhilstRunningForwards(Steps position);
		void homeWhilstRunningBackwards(Steps position);

		char name[15];
		
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
			Steps systemBacklash = 1499; // default value based on observations
			Steps positionWithinBacklash = 0; // negative when going forwards
		} backlashControl;

		struct {
			bool liveHomingEnabled = false;
			Steps switchSize = 3721 * 128 / 128; // size between forwards and backwards start engagement. Default value here
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

		uint32_t lastTime = 0;
		Steps targetPosition = 0;
		Steps position = 0;
		bool switchesArmed = false;

		struct {
			struct SwitchSeen {
				bool seen = false;
				Steps stepCountFirstSeen = 0;
			};

			struct SwitchesSeen {
				SwitchSeen forwards;
				SwitchSeen backwards;
			};

			bool invertSwitches = false;
			SwitchesSeen switchesSeen; // written in interrupt. read/cleared in updateStepCount
			Steps stepCount = 0;

			// Consecutive-sample debounce run counters for the switch latch, reset whenever
			// the raw reading drops out of the wanted state. See enableInterrupt().
			volatile uint16_t fwRun = 0;
			volatile uint16_t bwRun = 0;
		} inInterrupt;

		FrameSwitchEvents frameSwitchEvents;

		bool interruptEnabled = false;
		MotionState currentMotionState;

		HealthStatus healthStatus;

		// Debounce window (µstep samples of agreement) for the switch latch in
		// enableInterrupt(). 1 = the original one-shot latch (unchanged behaviour for
		// HOME_SWITCH_LEGACY, which never touches this); fastHomeRoutine raises it for the
		// optical build's shallower, noisier dip flanks.
		volatile uint16_t switchLatchDebounce = 1;

#ifndef HOME_SWITCH_LEGACY
		// Cached optical calibration, warm-reused across fastHomeRoutine calls until a
		// failure or a >25% width drift clears it (0 = not calibrated -- forces a cold,
		// self-calibrating run). opticalWidthCached is the flag width measured AT
		// opticalThresholdCached (W_cal in HOME_ROUTINE_DESIGN.md), used to derive
		// width-relative gates so they auto-scale to whatever ring is actually attached.
		int16_t opticalThresholdCached = 0;
		Steps opticalWidthCached = 0;

		// Motor generation (32:1 original vs 16:1 2026 module), auto-detected by
		// fastHomeRoutine on the first cold run each power-up from measured lead-to-lead
		// revolution distance -- RAM only, not persisted.
		const FastHomeParams * fastHomeParams = nullptr;
		bool fastHomeRatioConfirmed = false;
#endif
	};
}
