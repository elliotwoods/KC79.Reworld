#pragma once

#include "ofxSpout.h"

#include "Base.h"

namespace Modules {
	namespace Image {
		namespace Sources {
			class Spout : public Base
			{
			public:
				Spout();
				string getTypeName() const override;

				void init() override;
				void update() override;
				void populateInspector(ofxCvGui::InspectArguments&);

				void render(const RenderSettings&) override;
			protected:
				ofxSpout::Receiver spoutReceiver;
				ofTexture receiveTexture;
				ofFloatPixels receivePixels;
			};
		}
	}
}