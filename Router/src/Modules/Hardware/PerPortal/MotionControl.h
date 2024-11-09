#pragma once

#include "../../Base.h"
#include "msgpack11.hpp"
#include "../Utils.h"

namespace Modules {
	class Portal;
	
	namespace PerPortal {
		class MotionControl : public Base
		{
		public:
			MotionControl(Portal *, int axisIndex);
			string getTypeName() const override;
			string getGlyph() const override;

			string getName() const override;
			string getFWModuleName() const;

			void init();
			void update();
			void populateInspector(ofxCvGui::InspectArguments& args);
			void processIncoming(const nlohmann::json&) override;

			void move(Steps targetPosition);
			void move(Steps targetPosition
				, StepsPerSecond maxVelocity
				, StepsPerSecondPerSecond acceleration
				, StepsPerSecond minVelocity);

			void zeroCurrentPosition();

			msgpack11::MsgPack getMeasureSettings() const;
			void measureBacklash();
			void homeRoutine();

			bool getCurrentPositionKnown() const;
			Steps getCurrentPosition() const;

			bool getTargetPositionKnown() const;
			Steps getTargetPosition() const;

			void setReportedCurrentPosition(Steps);
			void setReportedTargetPosition(Steps);

			// actions to directly push sub-motion profiles
			void pushMotionProfile(int maxVelocity);
			void pushMotionProfile(int maxVelocity, int acceleration);
			void pushMotionProfile(int maxVelocity, int acceleration, int minVelocity);

			void testTimer();
			void deinitTimer();
			void initTimer();

			void pushMotionProfile();

		protected:
			Portal* portal;
			const int axisIndex;

			struct : ofParameterGroup {
				struct : ofParameterGroup {
					ofParameter<int> maxVelocity{ "Max velocity", 30000 };
					ofParameter<int> acceleration{ "Acceleration", 10000 };
					ofParameter<int> minVelocity{ "Min velocity", 100 };
					PARAM_DECLARE("MotionControl", maxVelocity, acceleration, minVelocity);
				} motionProfile;

				struct : ofParameterGroup {
					ofParameter<int> timeout_s{ "Timeout [s]", 60, 1, 120 };
					ofParameter<int> slowSpeed{ "Slow Speed [Hz]", 1000, 100, 1000000 };
					ofParameter<int> backOffDistance{ "Back off distance [steps]", 100, 1, 1000000 };
					ofParameter<int> debounceDistance{ "Debounce distance [steps]", 10, 1, 1000000 };
					PARAM_DECLARE("Measure settings", timeout_s, slowSpeed, backOffDistance, debounceDistance);
				} measureSettings;

				PARAM_DECLARE("MotionControl", motionProfile, measureSettings);
			} parameters;

			struct {
				int maxVelocity = -1;
				int acceleration = -1;
				int minVelocity = -1;
			} cachedSentParameters;

			struct {
				Utils::ReportedState<int32_t> position{ "position" };
				Utils::ReportedState<int32_t> targetPosition{ "targetPosition" };
				vector<Utils::IReportedState*> variables{
					&position
					, &targetPosition
				};
			} reportedState;
		};
	}
}