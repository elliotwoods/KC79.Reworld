#pragma once
#include "Base.h"

namespace Modules {
	namespace Patterns {
		class Lens : public Base {
		public:
			void init() override;
			string getTypeName() const override;
			string getGlyph() const override;
			void populateInspector(ofxCvGui::InspectArguments&);

			glm::vec2 calculate(glm::vec2& positionSNorm) override;

			float getAmplitude() const;

			struct : ofParameterGroup {
				struct : ofParameterGroup {
					ofParameter<float> max{ "Max", 1.0f, 0.0f, 2.0f };
					ofParameter<float> frequency{ "Frequency", 2.0f, 0.0f, 16.0f };
					PARAM_DECLARE("Amplitude", max, frequency)
				} amplitude;

				ofParameter<float> power{ "Power", 1.0f, 0.0f, 3.0f };

				PARAM_DECLARE("Lens", amplitude, power);
			} parameters;
		};
	}
}