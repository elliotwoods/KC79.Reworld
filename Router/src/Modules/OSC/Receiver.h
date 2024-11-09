#pragma once

#include "../Base.h"

#include "ofxOsc.h"
#include "OSC/Routes.h"

namespace Modules {
	namespace OSC {
		class Receiver : public Base
		{
		public:
			Receiver();

			string getTypeName() const override;
			void init() override;
			void update() override;
			void populateInspector(ofxCvGui::InspectArguments& args);
			ofxCvGui::PanelPtr getMiniView();
		protected:
			struct : ofParameterGroup {
				ofParameter<bool> enabled{ "Enabled", true };
				ofParameter<int> port{ "Port", 4000 };
				PARAM_DECLARE("OSC", enabled, port);
			} parameters;

			shared_ptr<ofxOscReceiver> oscReceiver;
		};
	}
}