#pragma once

#include "../Base.h"
#include "Constants.h"
#include "../Utils.h"

namespace Modules {
	class Portal;
	namespace PerPortal {
		class MotorDriverSettings : public Base
		{
		public:
			MotorDriverSettings(Portal *);
			string getTypeName() const override;
			string getGlyph() const override;
			
			void init() override;
			void update() override;
			void populateInspector(ofxCvGui::InspectArguments&);
			void pushValues();
			void pushCurrent();
			void pushMicrostepResolution();

			Steps getMicrostep() const;

		protected:
			Portal * portal;

			struct : ofParameterGroup {
				ofParameter<bool> autoPush{ "Auto push", true };
				ofParameter<float> current{ "Current", 0.15f, 0.0f, 0.3f };
				ofParameter<int> microstepResolution{ "Microstep resolution", 128, 1, 256 };
				PARAM_DECLARE("MotorDriverSettings", autoPush, current, microstepResolution);
			} parameters;

			struct : ofParameterGroup {
				float current = -1;
				int microstepResolution = -1;
			} cachedSentValues;
		};
	}
}