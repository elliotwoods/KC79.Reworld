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

			class Base : public Modules::Base
			{
			public:
				Base();
				void allocate(const RenderSettings&);
				virtual void render(const RenderSettings&) = 0;
				void updatePreview();
				shared_ptr<ofxCvGui::Widgets::Button> getButton(shared_ptr<Base>);

				bool getVisible() const;
				bool getRenderEnabled() const;
				float getAlpha() const;

				ofFloatPixels pixels;
				ofTexture preview;
			protected:
				struct : ofParameterGroup {
					ofParameter<bool> visible{ "Visible", true };
					ofParameter<bool> renderEnabled{ "Render enabled", true };
					ofParameter<float> alpha{ "Alpha" , 1.0f, 0.0f, 1.0f };
					PARAM_DECLARE("Base", visible, renderEnabled, alpha);
				} parameters;
			};
		}
	}
}