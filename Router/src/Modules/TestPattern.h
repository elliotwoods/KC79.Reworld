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
		void homeAndZero();
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
				ofParameter<bool> enabled{ "Enabled", false };
				ofParameter<float> period{ "Period", 5 * 60 };
				PARAM_DECLARE("Unwind", enabled, period);
			} unwind;

			struct : ofParameterGroup {
				ofParameter<bool> active{ "Active", false };
				struct : ofParameterGroup {
					ofParameter<bool> enabled{ "Enabled", true };
					ofParameter<float> duration_s{ "Duration [s]", 60, 1, 120 };
					ofParameter<float> period_m{ "Period [m]", 60, 1, 120 };
					PARAM_DECLARE("Timer", enabled, duration_s, period_m);
				} timer;
				PARAM_DECLARE("Home and zero", active, timer);
			} homeAndZero;

			PARAM_DECLARE("TestPattern", enabled, everyNFrames, wave, unwind, homeAndZero);
		} parameters;

		chrono::system_clock::time_point lastUnwind = chrono::system_clock::now();
		chrono::system_clock::time_point lastHomeAndZeroActiveStart = chrono::system_clock::now();
	};
}