#include "pch_App.h"
#include "Base.h"
#include "Modules/App.h"

namespace Modules {
	namespace Patterns {
		//----------
		Base::Base()
		{
			this->onPopulateInspector += [this](ofxCvGui::InspectArguments& args) {
				this->populateInspector(args);
			};
		}

		//----------
		string
			Base::getGlyph() const
		{
			return u8"\uf551";
		}

		//----------
		void
			Base::populateInspector(ofxCvGui::InspectArguments& args)
		{
			auto inspector = args.inspector;

			inspector->addParameterGroup(this->baseParameters);
			inspector->addLiveValue<float>("Phase", [this]() {
				return this->phase;
				})->onDraw.addListener([this](ofxCvGui::DrawArguments& args) {
					ofPushStyle();
					{
						ofSetColor(100);
						ofDrawRectangle(0
							, 0
							, args.localBounds.width * this->phase
							, args.localBounds.height);
					}
					ofPopStyle();
					}, this, -1);

			// preview the positions
			inspector->add(App::X()->getPositionsPreview());
		}
		//----------
		bool Base::isEnded(float time)
		{
			return time > this->baseParameters.duration.get();
		}

		//----------
		void
			Base::setTime(float time)
		{
			this->phase = time / this->baseParameters.duration.get();
		}
	}
}