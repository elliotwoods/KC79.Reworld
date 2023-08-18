#pragma once

#include "../Base.h"
#include "Constants.h"
#include "../Utils.h"

namespace Modules {
	class Portal;
	namespace PerPortal {
		class Pilot : public Base
		{
		public:
			Pilot(Portal *);
			string getTypeName() const override;
			string getGlyph() const override;

			void init() override;
			void update() override;

			void populateInspector(ofxCvGui::InspectArguments&);

			ofxCvGui::PanelPtr getPanel();
		protected:
			void pushValues();

			Portal * portal;

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
					ofParameter<float> a{ "A", 0, 0, 1 };
					ofParameter<float> b{ "B", 0, 0, 1 };
					ofParameter<float> offset{ "Offset", 0, -0.25, 0.25 };
					ofParameter<int> microstepsPerPrismRotation{ "Microsteps per prism rotation", MOTION_STEPS_PER_PRISM_ROTATION * 128 };
					ofParameter<bool> sendPeriodically{ "Send periodically", true };
					PARAM_DECLARE("Axes", a, b, offset, microstepsPerPrismRotation, sendPeriodically);
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
}