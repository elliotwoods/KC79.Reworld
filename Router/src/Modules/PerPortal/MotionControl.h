#pragma once

#include "../Base.h"

namespace Modules {
	class Portal;
	
	typedef int32_t Steps;

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

			void move(Steps position);

			void pushMotionProfile();

		protected:
			Portal* portal;
			const int axisIndex;

			struct : ofParameterGroup {
				ofParameter<int> maxVelocity{ "Max velocity", 30000 };
				ofParameter<int> acceleration{ "Acceleration", 10000 };
				ofParameter<int> minVelocity{ "Min velocity", 100 };
				PARAM_DECLARE("MotionControl", maxVelocity, acceleration, minVelocity);
			} parameters;

			struct {
				int maxVelocity = -1;
				int acceleration = -1;
				int minVelocity = -1;

			} cachedSentParameters;
		};
	}
}