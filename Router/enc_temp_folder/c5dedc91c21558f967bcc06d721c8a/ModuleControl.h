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

		void runTestRoutine(RS485::Target target, Axis);
		void runTestTimer(RS485::Target target, Axis);
	protected:
		weak_ptr<RS485> rs485;

		struct : ofParameterGroup {
			struct : ofParameterGroup {
				ofParameter<int> targetID{ "Target ID", 16 };
				struct : ofParameterGroup {
					ofParameter<float> current{ "Current", 0.1f, 0.0f, 0.3f };
					ofParameter<int> microstepResolution{ "Microstep resolution", 256, 1, 256 };
					PARAM_DECLARE("motorDriverSettings", current, microstepResolution);
				} motorDriverSettings;

				struct timerTest : ofParameterGroup {
					ofParameter<int> period{ "Period [us]", 1000, 10, 10000 };
					ofParameter<int> count{ "Count", 1000, 10, 10000 };
					ofParameter<bool> normaliseParameters{ "Normalise parameters", true };
					PARAM_DECLARE("testTimer", period, count, normaliseParameters);
				} testTimer;

				PARAM_DECLARE("Debug", targetID, motorDriverSettings, testTimer);
			} debug;
			PARAM_DECLARE("ModuleControl", debug)
		} parameters;

		struct {
			struct {
				struct {
					float current = -1;
					int microstepResolution = -1;
				} motorDriverSettings;
			} cachedValues;
		} debug;
	};
}