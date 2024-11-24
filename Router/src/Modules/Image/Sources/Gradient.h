#pragma once

#include "Base.h"

namespace Modules {
	namespace Image {
		namespace Sources {
			class Gradient : public Base
			{
			public:
				MAKE_ENUM(GradientType
					, (Radial, Horizontal, Vertical)
					, ("Radial", "Horizontal", "Vertical")
				);

				MAKE_ENUM(Wave
					, (Triangle, Sine, Sawtooth)
					, ("Triangle", "Sine", "Sawtooth")
				);

				Gradient();
				string getTypeName() const override;

				void init();
				void populateInspector(ofxCvGui::InspectArguments&);

				void render(const RenderSettings&) override;
			protected:

				struct : ofParameterGroup {
					ofParameter<GradientType> gradientType{ "Gradient type", GradientType::Radial };
					ofParameter<Wave> wave{ "Wave", Wave::Sine };
					ofParameter<float> frequency{ "Frequency", 1.0f };
					ofParameter<float> speed{ "Speed", 0.05f };
					ofParameter<glm::vec2> value1{ "Value 1", glm::vec2(0.0f, 0.0f) };
					ofParameter<glm::vec2> value2{ "Value 2", glm::vec2(1.0f, 1.0f) };
					PARAM_DECLARE("Gradient", gradientType, wave, frequency, speed, value1, value2);
				} parameters;
			};
		}
	}
}