#pragma once

#include "Base.h"
#include "ModuleControl.h"

#define MOTION_STEPS_PER_MOTOR_ROTATION 32
#define MOTION_STEPPER_GEAR_REDUCTION 9759 / 296
#define MOTION_GEAR_DRIVE 21
#define MOTION_GEAR_RING 118

#define MOTION_STEPS_PER_PRISM_ROTATION ( MOTION_STEPS_PER_MOTOR_ROTATION \
	* MOTION_GEAR_RING \
	* MOTION_STEPPER_GEAR_REDUCTION \
	/ MOTION_GEAR_DRIVE )

namespace Modules {
	class PortalPilot : public Base
	{
	public:
		PortalPilot(shared_ptr<ModuleControl>);

		string getTypeName() const override;
		void init() override;
		void update() override;

		void populateInspector(ofxCvGui::InspectArguments&);

		ofxCvGui::PanelPtr getPanel();
	protected:
		void pushValues();

		shared_ptr<ModuleControl> moduleControl;

		struct : ofParameterGroup {
			struct : ofParameterGroup {
				ofParameter<float> x{ "X", 0, -1, 1 };
				ofParameter<float> y{ "Y", 0, -1, 1 };
				ofParameter<bool> enabled{ "Enabled", true };
				PARAM_DECLARE("Position", x, y, enabled);
			} position;

			struct : ofParameterGroup {
				ofParameter<float> r{ "r", 0, 0, 1 };
				ofParameter<float> theta{ "Theta", 0, -acos(0), acos(0) };
				ofParameter<bool> enabled{ "Enabled", true };
				PARAM_DECLARE("Polar", r, theta, enabled);
			} polar;

			struct : ofParameterGroup {
				ofParameter<int> microstepsPerPrismRotation{ "Microsteps per prism rotation", MOTION_STEPS_PER_PRISM_ROTATION * 128};
				ofParameter<float> a{ "A", 0, 0, 1 };
				ofParameter<float> b{ "B", 0, 0, 1 };
				ofParameter<float> offset{ "Offset", 0, -0.25, 0.25 };
				PARAM_DECLARE("Axes", microstepsPerPrismRotation, a, b, offset);
			} axes;
			PARAM_DECLARE("PortalPilot", position, polar, axes);
		} parameters;

		struct : ofParameterGroup {
			float a = -2;
			float b = -2;
			chrono::system_clock::time_point lastUpdate = chrono::system_clock::now();
			chrono::milliseconds updatePeriod{ 1000 };
		} cachedSentValues;
	};
}