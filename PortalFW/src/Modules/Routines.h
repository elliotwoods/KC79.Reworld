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
		Exception tuneCurrent(const MotionControl::MeasureRoutineSettings& = MotionControl::MeasureRoutineSettings());
		Exception calibrate(const MotionControl::MeasureRoutineSettings& = MotionControl::MeasureRoutineSettings());
		Exception home(const MotionControl::MeasureRoutineSettings& = MotionControl::MeasureRoutineSettings());
		Exception measureCycle(const MotionControl::MeasureRoutineSettings& = MotionControl::MeasureRoutineSettings());
		
		void flashLEDs(uint16_t period, uint16_t count);

		// return false if already at max current
		bool stepUpCurrent();
	protected:
		App * app;

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