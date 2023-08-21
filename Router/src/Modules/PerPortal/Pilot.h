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
			MAKE_ENUM(LeadingControl
				, (Position, Polar, Axes)
				, ("Position", "Polar", "Axes")
			);

			Pilot(Portal *);
			string getTypeName() const override;
			string getGlyph() const override;

			void init() override;
			void update() override;

			void populateInspector(ofxCvGui::InspectArguments&);

			ofxCvGui::PanelPtr getPanel();

			void seeThrough();

			const glm::vec2 getPosition() const;
			const glm::vec2 getPolar() const;
			const glm::vec2 getAxes() const;

			void setPosition(const glm::vec2&);
			void setPolar(const glm::vec2&);
			void setAxes(const glm::vec2&);

			glm::vec2 positionToPolar(const glm::vec2&) const;
			glm::vec2 polarToPosition(const glm::vec2&) const;
			glm::vec2 polarToAxes(const glm::vec2&) const;
			glm::vec2 axesToPolar(const glm::vec2&) const;

			Steps axisToSteps(float, int axisIndex) const;
			float stepsToAxis(Steps, int axisIndex) const;
		protected:
			void pushValues();

			Portal * portal;

			struct : ofParameterGroup {
				ofParameter<LeadingControl> leadingControl{ "Leading control", LeadingControl::Position };

				struct : ofParameterGroup {
					ofParameter<float> x{ "X", 0, -1, 1 };
					ofParameter<float> y{ "Y", 0, -1, 1 };
					PARAM_DECLARE("Position", x, y);
				} position;

				struct : ofParameterGroup {
					ofParameter<float> r{ "r", 0, -1, 1 };
					ofParameter<float> theta{ "Theta", 0, -acos(0) * 2, acos(0) * 2 };
					PARAM_DECLARE("Polar", r, theta);
				} polar;

				struct : ofParameterGroup {
					ofParameter<float> a{ "A", 0, 0, 1 };
					ofParameter<float> b{ "B", 0, 0, 1 };
					ofParameter<float> offset{ "Offset", 0, -0.25, 0.25 };
					ofParameter<int> microstepsPerPrismRotation{ "Microsteps per prism rotation", MOTION_STEPS_PER_PRISM_ROTATION * 128 };
					ofParameter<bool> sendPeriodically{ "Send periodically", false };
					PARAM_DECLARE("Axes", a, b, offset, microstepsPerPrismRotation, sendPeriodically);
				} axes;
				PARAM_DECLARE("PortalPilot", leadingControl, position, polar, axes);
			} parameters;

			struct : ofParameterGroup {
				float a = -2;
				float b = -2;
				chrono::system_clock::time_point lastUpdateRequest = chrono::system_clock::now();
				chrono::milliseconds updatePeriod{ 1000 };
			} cachedSentValues;

			glm::vec2 liveAxisValues;
			glm::vec2 liveAxisTargetValues;
		};
	}
}