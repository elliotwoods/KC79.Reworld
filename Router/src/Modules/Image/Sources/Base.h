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
				void allocate(const RenderSettings&);
				virtual void render(const RenderSettings&) = 0;
				void updatePreview();
				shared_ptr<ofxCvGui::Widgets::Button> getButton(shared_ptr<Base>);

				ofFloatPixels pixels;
				ofTexture preview;
			};
		}
	}
}