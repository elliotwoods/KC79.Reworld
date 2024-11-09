#pragma once
#include "Base.h"

namespace Modules {
	namespace Patterns {
		class Swing : public Base {
		public:
			void init() override;
			string getTypeName() const override;
			string getGlyph() const override;
			void populateInspector(ofxCvGui::InspectArguments&);

			glm::vec2 calculate(glm::vec2& positionSNorm) override;

			float getAmplitude() const;
			float getRotationRad() const;

			struct : ofParameterGroup {
				struct : ofParameterGroup {
					ofParameter<float> max{ "Max", 1.0f, 0.0f, 2.0f };
					ofParameter<float> frequency{ "Frequency", 1.0f, 0.0f, 16.0f};
					PARAM_DECLARE("Amplitude", max, frequency)
				} amplitude;

				struct : ofParameterGroup {
					ofParameter<float> frequency{ "Frequency", 6.0f, 0.0f, 16.0f };
					PARAM_DECLARE("Rotation", frequency)
				} rotation;

				PARAM_DECLARE("Swing", amplitude, rotation);
			} parameters;
		};
	}
}