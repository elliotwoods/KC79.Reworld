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

		void flashLEDs(uint16_t period, uint16_t count);

		// return false if already at max current
		bool stepUpCurrent();
	protected:
		App * app;
	};
}