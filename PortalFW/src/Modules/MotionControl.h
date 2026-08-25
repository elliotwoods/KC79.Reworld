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
#ifdef HOME_SWITCH_LEGACY
			uint8_t timeout_s = 120;
#else
			// The optical cold path is genuinely long: three settled background probes (each
			// several RC settles), a revolution of seek, another revolution to identify the
			// gearbox, then a threshold band scan that re-measures the flag width at each step.
			// Measured against the real board that lands close enough to 120 s that a slightly
			// slow run would time out mid-scan and be retried from scratch, which is both
			// slower and much harder to read than simply allowing the scan to finish. This is a
			// ceiling, not a duration -- a warm home still completes in a few seconds and is
			// unaffected. uint8_t, so 255 is the hard maximum.
			uint8_t timeout_s = 240;
#endif
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

		// ---- unjam -------------------------------------------------------------------------
		//
		// A jammed prism is a mechanical problem, so the sweep that frees one is deliberately
		// crude: full steps at full torque, N forwards then N/2 back, over and over, until the
		// net travel adds up to `rotations` whole prism revolutions. The rocking is what does
		// the work -- a jam that a steady push cannot pass will often yield to repeated
		// reversals -- and the 2:1 ratio means every cycle still makes net progress, so the
		// sweep terminates on distance rather than on hope.
		//
		// It is also the module's coarsest instrument. See UnjamReport.
		struct UnjamSettings {
			// N, in FULL steps (the routine runs in full-step mode). 0 asks for a quarter of a
			// prism revolution, which is what getMicrostepsPerPrismRotation() reports once the
			// microstep resolution is _1.
			Steps stride = 0;

			// How much NET travel to clear, in whole prism revolutions.
			uint8_t rotations = 10;

			// Full steps per second, and the ramp that reaches it. 523 is the speed the
			// previous unjam routine used and is kept unchanged: in full-step mode that is
			// already ~980 motor RPM and there is no torque to spare above it.
			//
			// The acceleration is NOT free to be small. calculateMotionState computes
			// `acceleration * dt_us / 1000000` in integer arithmetic, so a tick shorter than
			// 1000000/acceleration microseconds accelerates by zero and the ramp never starts.
			// The old routine paid for its acceleration of 500 with a 20 ms tick, which is far
			// too coarse to catch a home flag only ~9 full steps wide. 1500 needs 667 us and
			// is safe at the 2 ms tick this routine runs.
			StepsPerSecondPerSecond acceleration = 1500;
			StepsPerSecond speed = 523;

			// Ceiling for the whole sweep, in seconds. Deliberately not
			// MeasureRoutineSettings::timeout_s: ten revolutions of NET travel is thirty
			// revolutions of movement, which at 523 full steps/s cannot fit in that field's
			// uint8_t range at all.
			uint16_t timeout_s = 900;
		};

		// What the sweep saw of the home flag on the way past it.
		//
		// This is what makes unjam a diagnostic and not just a repair. The optical flag passes
		// the sensor exactly once per prism revolution, so the motor-step distance between
		// consecutive sightings IS one prism revolution as the motor measures it. A prism that
		// is turning cleanly reports the same number every lap, and that number is
		// getMicrostepsPerPrismRotation(). A prism that is slipping -- or a gearbox that is
		// binding and losing steps -- reports gaps that are longer, and reports them
		// inconsistently. `worstDeviation` is therefore the answer to "is this axis actually
		// moving the prism", which no other routine reports as a number.
		//
		// Every distance here is in FULL steps, the unit the sweep runs in.
		struct UnjamReport {
			bool completed = false;      // reached the requested net travel
			bool ran = false;            // begin() got as far as moving
			uint16_t cycles = 0;
			Steps netTravel = 0;         // net forward progress
			Steps expectedGap = 0;       // one prism revolution, in full steps
			uint8_t sightings = 0;
			Steps firstSighting = 0;
			Steps lastSighting = 0;
			Steps shortestGap = 0;
			Steps longestGap = 0;
			Steps worstDeviation = 0;    // largest |gap - expectedGap|
			int32_t totalAbsDeviation = 0;
		};

		// Warning : This routine loses homing -- it ends with the datum zeroed, because a sweep
		// that deliberately drives through a jam cannot know where it finished.
		//
		// Two overloads rather than a defaulted argument: `= UnjamSettings()` inside the class
		// needs UnjamSettings' member initialisers to be complete, and they are not until the
		// closing brace.
		Exception unjamRoutine(const MeasureRoutineSettings&);
		Exception unjamRoutine(const MeasureRoutineSettings&, const UnjamSettings&);

		// The same sweep, split into three so that two axes can run it AT THE SAME TIME.
		// Routines::unjam drives both; unjamRoutine is these three around one loop.
		//
		// Two things are deliberately left to the caller. The shared MotorDriverSettings --
		// one VREF pin and one microstep pair feed both axes -- must be taken to full torque
		// and full steps once, by the caller, before either axis begins: if each axis captured
		// its own "prior" the second would capture the first's override and restore the wrong
		// operating point. And App::updateFromRoutine() is the caller's to call, because in
		// the two-axis case doing it inside unjamUpdate would run it twice per tick.
		Exception unjamBegin(const MeasureRoutineSettings&, const UnjamSettings&);
		bool unjamUpdate();              // false once this axis has stopped working
		Exception unjamEnd();
		const UnjamReport & getUnjamReport() const;

		// ---- cycle check ------------------------------------------------------------------
		//
		// The cheapest honest question you can ask a module: does this prism turn, and does its
		// home flag come past exactly once per revolution? Nothing else in startup answers it
		// quickly. Calibration is expensive precisely because it is trying to work out an
		// operating point, and on an axis that cannot turn its prism it spends minutes doing
		// that before failing -- so this runs first, on BOTH axes at once, and startup does not
		// pay for calibration until both have passed.
		//
		// "Exactly once" is the whole point, and it is stricter than it sounds. An axis whose
		// sensor fires twice a lap has an optical problem that homing will misread as a datum,
		// and it is invisible to a check that only asks "did the flag come back". So a second
		// sighting arriving EARLY is a failure in its own right, reported as soon as it lands.
		struct CycleCheckSettings {
			// Traverse speed, in microsteps per second.
			//
			// Not a free choice, and not the fast-home seek speed. Raising the traverse to
			// 24,000 once cut startup from 105 s to 79 s and made a GOOD module fail, because
			// microsteps slipped at that speed are indistinguishable from gear error: axis A
			// measured 12 full steps out against a 10-step tolerance, where at this speed it
			// reads 5928 +/- 2. The measurement is only worth making at a speed it survives.
			StepsPerSecond speed = MOTION_DEFAULT_SPEED;

			// How far a revolution may be from MOTION_STEPS_PER_PRISM_ROTATION, in FULL steps.
			Steps tolerance = MOTION_ALLOWED_PRISM_ROTATION_ERROR;

			// Ceiling for one axis's check. Generous: the work is bounded by distance, not by
			// this, and a check that dies on the clock tells you less than one that finishes.
			uint16_t timeout_s = 60;
		};

		enum class CycleCheckVerdict : uint8_t {
			NotRun = 0,
			Working,
			Passed,
			NoFlag,        // nothing in a revolution and a bit -- jammed, or optically dead
			ExtraFlag,     // a second sighting arrived early: more than one feature per lap
			WrongSpacing,  // the flag came back, but not a revolution later
			Timeout,
			Aborted
		};

		// Split three ways so Routines::cycleCheck can step both axes together, exactly as
		// unjamBegin/Update/End are. The blocking helpers (routineMoveTo,
		// routineMoveToUntilSeeSwitch, routineFindSwitchAccurate) each call
		// App::updateFromRoutine() themselves, which is why none of them can be used here.
		//
		// Unlike unjam, this one owns nothing shared: it runs at the module's normal current
		// and microstepping, and leaves the threshold DAC at its power-on value -- which is
		// what lets both axes run it at once. One DAC feeds both comparators, so a check that
		// tried to pick a per-axis threshold could not be simultaneous. This one does not need
		// to: it is asking about geometry, not about optics.
		Exception cycleCheckBegin(const CycleCheckSettings&);
		bool cycleCheckUpdate();          // false once this axis has stopped working
		Exception cycleCheckEnd();
		CycleCheckVerdict getCycleCheckVerdict() const;
		Steps getCycleCheckSpacing() const;   // microsteps between the two sightings
		static const char * cycleCheckVerdictName(CycleCheckVerdict);
#ifdef HOME_SWITCH_LEGACY
		// Mechanical (PCB v4) only -- see the definitions.
		Exception measureBacklashRoutine(const MeasureRoutineSettings&);
		Exception homeRoutine(const MeasureRoutineSettings&);
#endif
#ifndef HOME_SWITCH_LEGACY
		enum class FastHomeFailure : uint8_t {
			None = 0,
			Aborted,
			Timeout,
			Motion,
			FeatureMissing,
			FeatureTooWide,
			SpeedDependentEdge,
			SensorUnstable,
			OpticalContrast,
			Backlash
		};

		// Self-calibrating optical home: replaces measureBacklashRoutine + homeRoutine for
		// the optical switch in one pass. See HomeSwitchTest/portalfw_port/PORTING.md and
		// HomeSwitchTest/reports/newring/HOME_ROUTINE_DESIGN.md for the bench provenance.
		Exception fastHomeRoutine(const MeasureRoutineSettings&);
		FastHomeFailure getLastFastHomeFailure() const { return this->lastFastHomeFailure; }
		int16_t getOpticalThreshold() const { return this->opticalThresholdCached; }
		Steps getOpticalWidth() const { return this->opticalWidthCached; }
		void restoreOpticalCalibration(uint8_t threshold, Steps width);

		// True iff a (threshold, width) pair sits in the clean operating band -- see the
		// FASTHOME_T_OP_* / FASTHOME_W_* constants. The single source of truth for "is this
		// operating point trustworthy". Static so the persist path (App) can screen a pair
		// before it ever reaches flash.
		static bool opticalPointPlausible(int thresholdT, Steps widthW);

		// ---- optical front-end diagnostics -------------------------------------------------
		// Neither of these moves the motor, so both are safe to call outside a routine. They
		// exist because the sensor's world is not a constant: the reflection profile shifts
		// unit to unit and day to day, and on the production ring the background never crosses
		// at any threshold at all. Before trusting fastHomeRoutine's gates on a given board you
		// have to be able to see what its sensor actually reports -- which nothing else in the
		// firmware exposes.

		// The live comparator output at whatever the shared threshold DAC is currently set to.
		bool getHomeSwitchActive() const;

		// Settled binary search for the comparator crossing duty at the CURRENT position, using
		// the same probe fastHomeRoutine calibrates with (real RC settle per step -- a fast
		// sweep reads ~20 counts high and must not be used to pick a threshold). Returns the
		// crossing duty, -1 if the sensor is railed across the whole sweep (railLo says which
		// way: LOW/dark vs HIGH/bright), or -2 on abort/timeout. Leaves the shared DAC at the
		// last probed value; the caller restores it.
		int probeHomeCrossing(bool & railLo, uint32_t timeoutTime);

		// One revolution at a FIXED settled threshold, reporting every comparator transition the
		// homing latch sees on the way round.
		//
		// This is the instrument, and the only one whose answer may be used to choose an
		// operating threshold. It is a fixed, fully-settled threshold read by the moving
		// comparator with the same debounced latch homing uses -- i.e. it measures exactly what
		// homing will experience. Every cheaper alternative has been shown to lie on this
		// hardware: a swept DAC reads 10-20 counts high because of the ~100 ms RC, and a settled
		// binary-search probe has returned "no crossing" while parked on a flag a census found
		// on every lap. Trust order is census > static level ladder > grid scan > settled probe.
		//
		// Moves the motor a full revolution and LOSES nothing but time -- it does not touch the
		// datum, the backlash model or the cached calibration. Restores the threshold, the
		// motion profile and the drive current on every exit path.
		Exception homeSwitchCensusRoutine(uint8_t threshold
			, StepsPerSecond speed
			, const MeasureRoutineSettings&);
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
				volatile bool seen = false;
				Steps stepCountFirstSeen = 0;
			};

			struct SwitchesSeen {
				SwitchSeen forwards;
				SwitchSeen backwards;
			};

			bool invertSwitches = false;
			SwitchesSeen switchesSeen; // written in interrupt. read/cleared in updateStepCount

			// volatile because the comment above is true and the compiler cannot see it. This
			// survived without the qualifier only because the non-inlined calls around every
			// access happened to force reloads -- which is a property of this optimiser at this
			// version, not of the code. The toolchain is pinned partly for reasons like this,
			// but a latent miscompile is not something to leave resting on a pin.
			volatile Steps stepCount = 0;

			// Consecutive-sample debounce run counters for the switch latch, reset whenever
			// the raw reading drops out of the wanted state. See enableInterrupt().
			volatile uint16_t fwRun = 0;
			volatile uint16_t bwRun = 0;
		} inInterrupt;

		FrameSwitchEvents frameSwitchEvents;

		// Everything unjamBegin/unjamUpdate/unjamEnd carry between ticks. It lives here rather
		// than on the stack of a routine function because the two-axis form of the sweep is a
		// state machine stepped from Routines::unjam, not a loop that owns its own frame.
		struct UnjamState {
			bool active = false;
			UnjamSettings settings;
			uint32_t deadline = 0;
			Steps stride = 0;
			Steps travelTarget = 0;      // full steps of net progress wanted
			Steps origin = 0;            // position when the sweep began
			Steps nextRevolutionMark = 0;
			bool forwards = true;
			bool timedOut = false;

			// Sighting acceptance. The interrupt latch re-arms every frame, so a flag that
			// stays active for ~9 full steps would otherwise be counted once per frame for as
			// long as we are on it; and a forward leg that STARTS parked on the flag would
			// latch a rising edge that is not one. `offFlag` answers both: a sighting is only
			// accepted once the sensor has read inactive since the last one.
			bool offFlag = false;
			bool haveSighting = false;
			Steps lastSighting = 0;

			// Priors, restored by unjamEnd. The shared MotorDriverSettings are NOT here -- see
			// unjamBegin's contract.
			MotionProfile priorMotionProfile;
			Steps priorSystemBacklash = 0;
			Steps priorPositionWithinBacklash = 0;
			uint16_t priorLatchDebounce = 1;
			bool priorSwitchesArmed = false;

			UnjamReport report;
		};
		UnjamState unjamState;

		// Everything cycleCheckBegin/Update/End carry between ticks; see UnjamState above for
		// why the state lives on the object rather than in a routine's stack frame.
		struct CycleCheckState {
			bool active = false;
			CycleCheckSettings settings;
			uint32_t deadline = 0;

			Steps revolution = 0;        // microsteps per prism revolution
			Steps toleranceUsteps = 0;
			Steps sameFlagWindow = 0;
			Steps origin = 0;

			bool confirming = false;     // first sighting is in; measuring the lap
			bool parking = false;        // passed, backing up to leave the flag just ahead
			bool offFlag = false;        // sensor has read inactive since the last sighting

			Steps firstSighting = 0;
			Steps spacing = 0;
			CycleCheckVerdict verdict = CycleCheckVerdict::NotRun;

			MotionProfile priorMotionProfile;
			Steps priorSystemBacklash = 0;
			Steps priorPositionWithinBacklash = 0;
			uint16_t priorLatchDebounce = 1;
			bool priorSwitchesArmed = false;
		};
		CycleCheckState cycleCheckState;

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

		// Set once this axis has tried the compile-time default operating point and been unable
		// to home on it. From then on (until the next reset) fastHomeRoutine goes straight to
		// the full self-calibration instead of seeding, so a module whose flag does not match
		// the fleet default does not pay for the failed fast attempt on every retry.
		bool opticalDefaultRejected = false;

		// Machine-readable cause for the most recent fast-home attempt. Routines uses this to
		// decide whether extra motor current can plausibly help; optical disagreement must not
		// be disguised as a current-recovery success or persisted at 250 mA.
		FastHomeFailure lastFastHomeFailure = FastHomeFailure::None;
#endif
	};
}
