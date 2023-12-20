#pragma once
#include "../Base.h"

namespace Modules {
	namespace Patterns {
		class Base : public Modules::Base
		{
		public:
			Base();
			virtual string getGlyph() const override;
			virtual void update() override {};
			void populateInspector(ofxCvGui::InspectArguments&);

			bool isEnded(float time);
			void setTime(float);
			virtual glm::vec2 calculate(glm::vec2& positionSNorm) = 0;
			
			struct : ofParameterGroup {
				ofParameter<bool> enabled{ "Enabled", true };
				ofParameter<float> duration{ "Duration", 120.0f, 1.0f, 300.0f};
				PARAM_DECLARE("Base", enabled, duration);
			} baseParameters;

		protected:
			float phase = 0.0f;
		};
	}
}