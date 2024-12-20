#pragma once

#include "../../Base.h"
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

			// Check if the sent values are stale and need push
			bool needsPush();

			// Notify the Pilot that the values have been sent (e.g. in a Keyframe message)
			void notifyValuesSent();

			void populateInspector(ofxCvGui::InspectArguments&);

			ofxCvGui::PanelPtr getPanel();

			void seeThrough();

			const glm::vec2 getPosition() const;
			const glm::vec2 getPolar() const;
			const glm::vec2 getAxes() const;

			/// Set the target position
			void setPosition(const glm::vec2&);
			void setPolar(const glm::vec2&);
			void setAxes(const glm::vec2&);

			/// Reset the local targets
			void resetLocal();
			void unwind();

			/// Reset the local and cached remotes. You can call this after sending a home to the hardware
			void reset();

			glm::vec2 positionToPolar(const glm::vec2&) const;
			glm::vec2 polarToPosition(const glm::vec2&) const;
			glm::vec2 polarToAxes(const glm::vec2&) const;
			glm::vec2 axesToPolar(const glm::vec2&) const;

			glm::vec2 findClosestAxesCycle(const glm::vec2&) const;

			Steps axisToSteps(float, int axisIndex) const;
			float stepsToAxis(Steps, int axisIndex) const;

			void push();

			// Put a message into the outbox that calculates the most recent axis values when the message is sent
			void pushLazy();
			void pollPosition();

			glm::tvec2<Steps> getAxisSteps() const;
			glm::vec2 getLivePosition() const;
			glm::vec2 getLiveTargetPosition() const;
			bool isInTargetPosition() const;

			void takeCurrentPosition();

		protected:

			Portal * portal;

			struct : ofParameterGroup {
				ofParameter<LeadingControl> leadingControl{ "Leading control", LeadingControl::Axes };

				struct : ofParameterGroup {
					ofParameter<float> x{ "X", 0, -1, 1 };
					ofParameter<float> y{ "Y", 0, -1, 1 };
					PARAM_DECLARE("Position", x, y);
				} position;

				struct : ofParameterGroup {
					ofParameter<float> r{ "r", 0, -1, 1 };
					ofParameter<float> theta{ "Theta", 0, (float) - acos(0) * 2.0f, (float) acos(0) * 2.0f };
					PARAM_DECLARE("Polar", r, theta);
				} polar;

				struct : ofParameterGroup {
					ofParameter<float> a{ "A", 0, 0, 1 };
					ofParameter<float> b{ "B", 0, 0, 1 };
					ofParameter<bool> cyclic{ "Cyclic", true };
					ofParameter<float> offset{ "Offset", 0, -0.25, 0.25 };
					ofParameter<int> microstepsPerPrismRotation{ "Microsteps per prism rotation", MOTION_MICROSTEPS_PER_PRISM_ROTATION };
					ofParameter<bool> sendPeriodically{ "Send periodically", false };
					PARAM_DECLARE("Axes", a, b, cyclic, offset, microstepsPerPrismRotation, sendPeriodically);
				} axes;
				PARAM_DECLARE("PortalPilot", leadingControl, position, polar, axes);
			} parameters;

			struct : ofParameterGroup {
				float a = -2;
				float b = -2;
				chrono::system_clock::time_point lastUpdateRequest = chrono::system_clock::now();
				chrono::milliseconds updatePeriod{ 1000 };
			} cachedSentValues;


			glm::tvec2<bool> liveAxisValuesKnown{ false, false };
			glm::vec2 liveAxisValues;
			glm::tvec2<bool> liveAxisTargetValuesKnown{ false, false };
			glm::vec2 liveAxisTargetValues;
		};
	}
}