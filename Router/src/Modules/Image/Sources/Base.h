#pragma once

#include "../../Base.h"

namespace Modules {
	namespace Image {
		namespace Sources {
			struct RenderSettings {
				int width;
				int height;
				float time;
			};

			MAKE_ENUM(Style
				, (Direct, HV_ThetaR, Centered)
				, ("Direct", "HV_ThetaR", "Centered"));

			class Base : public Modules::Base
			{
			public:
				struct Parameters : ofParameterGroup {
					ofParameter<bool> visible{ "Visible", true };
					ofParameter<bool> renderEnabled{ "Render enabled", true };
					ofParameter<float> alpha{ "Alpha" , 1.0f, 0.0f, 1.0f };
					ofParameter<Style> style{ "Style", Style::Direct };
					PARAM_DECLARE("Base", visible, renderEnabled, alpha, style);
				};

				Base();
				void allocate(const RenderSettings&);
				virtual void render(const RenderSettings&) = 0;
				void updatePreview();
				shared_ptr<ofxCvGui::Widgets::Button> getButton(shared_ptr<Base>);

				const Parameters& getBaseParameters() const;

				ofFloatPixels pixels;
				ofTexture preview;
			protected:
				Parameters parameters;
			};
		}
	}
}