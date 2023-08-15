#pragma once

#include "Base.h"
#include "RS485.h"

namespace Modules {
	class ModuleControl : public Base
	{
	public:
		enum Axis {
			A
			, B
		};

		ModuleControl(shared_ptr<RS485>);

		string getTypeName() const override;
		void init() override;
		void update() override;

		void populateInspector(ofxCvGui::InspectArguments&);

		void setCurrent(RS485::Target target, float);
		void setMicrostepResolution(RS485::Target target, int);

		void zeroCurrentPosition(RS485::Target, Axis);
		void runTestRoutine(RS485::Target target, Axis);
		void runTestTimer(RS485::Target target, Axis);

		void move(RS485::Target target
			, Axis
			, int32_t targetPosition
			, int32_t maxVelocity
			, int32_t acceleration
			, int32_t minVelocity);

		void measureBacklash(RS485::Target
			, Axis);

	protected:
		weak_ptr<RS485> rs485;

		struct : ofParameterGroup {
			struct : ofParameterGroup {
				ofParameter<int> targetID{ "Target ID", 1 };
				struct : ofParameterGroup {
					ofParameter<float> current{ "Current", 0.15f, 0.0f, 0.3f };
					ofParameter<int> microstepResolution{ "Microstep resolution", 128, 1, 256 };
					PARAM_DECLARE("motorDriverSettings", current, microstepResolution);
				} motorDriverSettings;

				struct : ofParameterGroup {
					ofParameter<int> count{ "Count", 4000, 1, 10000 };
					ofParameter<int> period{ "Period [us]", 500, 10, 10000 };
					ofParameter<bool> normaliseParameters{ "Normalise parameters", true };
					PARAM_DECLARE("testTimer", count, period, normaliseParameters);
				} testTimer;

				struct : ofParameterGroup {
					ofParameter<int> targetPosition{ "Target position", 10000 };
					
					ofParameter<int> maxVelocity{ "Max velocity", 60000 };
					ofParameter<int> acceleration{ "Acceleration", 10000 };
					ofParameter<int> minVelocity{ "Min velocity", 1000 };

					ofParameter<bool> relativeMove{ "Relative move", true };
					ofParameter<int> movement{ "Movement", 10000 };

					struct : ofParameterGroup {
						ofParameter<bool> enabled{ "Enabled", false };
						ofParameter<float> position{ "Position", 0, 0, 10000};
						ofParameter<int> range{ "Range", 600000 };
						PARAM_DECLARE("Continuous motion", enabled, position, range);
					} continuousMotion;

					struct : ofParameterGroup {
						ofParameter<int> timeout_s{ "Timeout [s]", 30, 1, 120 };
						ofParameter<int> fastSpeed{ "Fast Speed [Hz]", 80000, 100, 1000000 };
						ofParameter<int> slowSpeed{ "Slow Speed [Hz]", 1000, 100, 1000000 };
						ofParameter<int> backOffDistance{ "Back off distance [steps]", 1000, 1, 1000000 };
						ofParameter<int> debounceDistance{ "Debounce distance [steps]", 2, 1, 1000000 };
						PARAM_DECLARE("Backlash measure", timeout_s, fastSpeed, slowSpeed, backOffDistance, debounceDistance);
					} measureBacklash;

					PARAM_DECLARE("Motion Control"
						, targetPosition
						, maxVelocity
						, acceleration
						, minVelocity
						, relativeMove
						, movement
						, continuousMotion
						, measureBacklash);
				} motionControl;

				PARAM_DECLARE("Debug", targetID, motorDriverSettings, testTimer, motionControl);
			} debug;
			PARAM_DECLARE("ModuleControl", debug)
		} parameters;

		struct {
			struct {
				struct {
					float current = -1;
					int microstepResolution = -1;
				} motorDriverSettings;

				struct {
					struct {
						int position;
					} continuousMove;
				} motionControl;
			} cachedValues;
		} debug;
	};
}