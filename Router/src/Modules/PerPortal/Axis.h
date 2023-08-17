#pragma once

#include "../Base.h"

#include "MotorDriver.h"
#include "MotionControl.h"

namespace Modules {
	class Portal;

	namespace PerPortal {
		class Axis : public Base
		{
		public:
			Axis(Portal *, int axisIndex);

			string getTypeName() const override;
			string getGlyph() const override;
			string getName() const override;

			void init() override;
			void update() override;
			void populateInspector(ofxCvGui::InspectArguments&);

			shared_ptr<MotorDriver> getMotorDriver();
			shared_ptr<MotionControl> getMotionControl();
		protected:
			Portal * portal;
			const int axisIndex;

			shared_ptr<MotorDriver> motorDriver;
			shared_ptr<MotionControl> motionControl;
		};
	}
}