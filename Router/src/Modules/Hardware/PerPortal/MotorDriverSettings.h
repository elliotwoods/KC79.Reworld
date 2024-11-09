#pragma once

#include "../../Base.h"
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

			void setCurrent(float milliAmps);

		protected:
			Portal * portal;

			struct : ofParameterGroup {
				ofParameter<bool> autoPush{ "Auto push", true };
				ofParameter<float> current{ "Current [A]", 0.150, 0, 0.300 };
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