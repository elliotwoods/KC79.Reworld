#pragma once

#include "Base.h"
#include "RS485.h"
#include "Utils.h"

#include "PerPortal/MotorDriverSettings.h"
#include "PerPortal/Axis.h"
#include "PerPortal/Pilot.h"
#include "PerPortal/Logger.h"

#include "../msgpack11/msgpack11.hpp"

namespace Modules {
	class App;

	class TestPattern : public Base
	{
	public:
		TestPattern(App*);
		string getTypeName() const override;
		string getGlyph() const override;

		void init() override;
		void update() override;

		void populateInspector(ofxCvGui::InspectArguments&);
	protected:
		void wave();
		void unwind();
		App* app;
		
		struct : ofParameterGroup {
			ofParameter<bool> enabled{ "Enabled", false };
			ofParameter<int> everyNFrames{ "Every N frames", 10 };

			struct : ofParameterGroup {
				ofParameter<float> period{ "Period", 120 };
				ofParameter<float> width{ "Width", 1.5f, 0.0f, 10.0f };
				ofParameter<float> height{ "Height", 3.0f, 0.0f, 10.0f };
				ofParameter<float> amplitude{ "Amplitude", 1.0f, 0.0f, 10.0f };
				PARAM_DECLARE("Wave", period, width, height, amplitude);
			} wave;

			struct : ofParameterGroup {
				ofParameter<bool> enabled{ "Enabled", true };
				ofParameter<float> period{ "Period", 5 * 60 };
				PARAM_DECLARE("Unwind", enabled, period);
			} unwind;

			PARAM_DECLARE("TestPattern", enabled, everyNFrames, wave, unwind);
		} parameters;

		chrono::system_clock::time_point lastUnwind = chrono::system_clock::now();
	};
}