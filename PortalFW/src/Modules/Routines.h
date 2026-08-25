#pragma once
#include "Base.h"
#include "../Exception.h"
#include "MotionControl.h"

namespace Modules {
	class App;

	class Routines : public Base {
	public:
		Routines(App * app);
		const char * getTypeName() const;

		void startup();

		Exception init(const MotionControl::MeasureRoutineSettings& = MotionControl::MeasureRoutineSettings());
		Exception unjam(const MotionControl::MeasureRoutineSettings& = MotionControl::MeasureRoutineSettings());
		Exception calibrate(const MotionControl::MeasureRoutineSettings& = MotionControl::MeasureRoutineSettings());
		Exception home(const MotionControl::MeasureRoutineSettings& = MotionControl::MeasureRoutineSettings());

		// The fast check: both axes turned at once, verifying that each home flag comes past
		// exactly once per revolution. Runs before any calibration, and refuses to pay for
		// calibration on a module that cannot turn its prisms. See
		// MotionControl::cycleCheckBegin for the measurement and why it is worth trusting.
		Exception cycleCheck(const MotionControl::MeasureRoutineSettings& = MotionControl::MeasureRoutineSettings());

		void flashLEDs(uint16_t period, uint16_t count);
	protected:
		App * app;
#ifdef HOME_SWITCH_LEGACY
		Exception homeAxisWithRecovery(MotionControl * motionControl
			, const MotionControl::MeasureRoutineSettings & settings);
#endif

#ifndef HOME_SWITCH_LEGACY
		// One axis's share of calibrate() for the optical switch -- see the definition for the
		// cold/warm retry policy. A member (not a free function) so it can reach
		// MotionControl's protected fastHomeRoutine()/opticalThresholdCached via the
		// `friend Routines;` grant in MotionControl.h.
		Exception calibrateAxisFastHome(MotionControl * motionControl
			, const MotionControl::MeasureRoutineSettings & settings);
#endif
	};
}
