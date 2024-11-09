#pragma once

#include "../../Base.h"
#include "Constants.h"

namespace Modules {
	class Portal;
	class Axis;
	namespace PerPortal {
		class MotorDriver : public Base
		{
		public:
			MotorDriver(Portal *, int axisIndex);

			string getTypeName() const override;
			string getGlyph() const override;

			void init() override;
			void update() override;
			void populateInspector(ofxCvGui::InspectArguments&);

			void testRoutine();
			void testTimer();
		protected:
			Portal * portal;
			Axis* axis;

			const int axisIndex;

			struct : ofParameterGroup {
				struct : ofParameterGroup {
					ofParameter<int> count{ "Count", 4000, 1, 10000 };
					ofParameter<int> period{ "Period [us]", 500, 10, 10000 };
					ofParameter<bool> normaliseParameters{ "Normalise parameters", true };
					PARAM_DECLARE("testTimer", count, period, normaliseParameters);
				} testTimer;
				PARAM_DECLARE("MotorDriver", testTimer);
			} parameters;
		};
	}
}