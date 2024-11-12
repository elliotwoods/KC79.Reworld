#pragma once

#include "../TopLevelModule.h"
#include "../../Utils.h"

#include "ofxOsc.h"
#include "OSC/Routes.h"

namespace Modules {
	namespace OSC {
		class Receiver : public TopLevelModule
		{
		public:
			Receiver();

			string getTypeName() const override;
			void init() override;
			void update() override;
			void populateInspector(ofxCvGui::InspectArguments& args);

			ofxCvGui::PanelPtr getMiniView() override;
			int getMiniViewHeight() const override;
		protected:
			struct : ofParameterGroup {
				ofParameter<bool> enabled{ "Enabled", true };
				ofParameter<int> port{ "Port", 4000 };
				PARAM_DECLARE("OSC", enabled, port);
			} parameters;

			shared_ptr<ofxOscReceiver> oscReceiver;
			Utils::IsFrameNew isFrameNew;
		};
	}
}