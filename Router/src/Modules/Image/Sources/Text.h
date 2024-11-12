#pragma once

#include "Base.h"

namespace Modules {
	namespace Image {
		namespace Sources {
			class Text : public Base
			{
			public:
				Text();
				string getTypeName() const override;

				void init() override;
				void update() override;
				void populateInspector(ofxCvGui::InspectArguments&);

				void render(const RenderSettings&) override;
			protected:
				ofFbo fbo;

				ofTrueTypeFont font;
				string loadedFontName;
				int loadedFontSize = 0;

				struct : ofParameterGroup {
					ofParameter<string> text{ "Text", "TEST" };
					ofParameter<string> font{ "Font", "" };
					ofParameter<int> size{ "Size", 11 };
					ofParameter<bool> usePixelFont{ "Use Pixel Font", false };	
					ofParameter<int> border{ "Border", 0 };
					ofParameter<bool> inverse{ "Inverse", false };
					PARAM_DECLARE("Text", text, font, size, usePixelFont, border, inverse);
				} parameters;
			};
		}
	}
}