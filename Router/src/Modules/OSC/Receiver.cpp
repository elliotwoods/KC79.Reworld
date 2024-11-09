#include "pch_App.h"
#include "Receiver.h"

namespace Modules {
	namespace OSC {
		//----------
		Receiver::Receiver()
		{

		}

		//----------
		string
			Receiver::getTypeName() const
		{
			return "OSC::Receiver";
		}

		//----------
		void
			Receiver::init()
		{

		}

		//----------
		void
			Receiver::update()
		{
			// Check if should close 
			if (this->oscReceiver && !this->parameters.enabled) {
				this->oscReceiver.reset();
			}

			// Check settings
			if (this->oscReceiver) {
				if (this->oscReceiver->getPort() != this->parameters.port) {
					this->oscReceiver.reset();
				}
			}

			// Open the device
			if (!this->oscReceiver && this->parameters.enabled) {
				this->oscReceiver = make_shared<ofxOscReceiver>();
				this->oscReceiver->setup(this->parameters.port);
				if (!this->oscReceiver->isListening()) {
					this->oscReceiver.reset();
				}
			}

			// Perform updates
			if (this->oscReceiver) {
				ofxOscMessage message;
				while (this->oscReceiver->getNextMessage(&message)) {
					::OSC::handleRoute(message);
				}
			}
		}

		//----------
		void
			Receiver::populateInspector(ofxCvGui::InspectArguments& args)
		{
			auto inspector = args.inspector;
			inspector->addIndicatorBool("OSC receiver open", [this]() {
				return (bool)this->oscReceiver;
				});
			inspector->addParameterGroup(this->parameters);
		}

		//----------
		ofxCvGui::PanelPtr
			Receiver::getMiniView()
		{
			auto view = make_shared<ofxCvGui::Panels::Base>();
			view->onDraw += [this](ofxCvGui::DrawArguments& args) {
				ofxCvGui::Utils::drawText(this->getName(), args.localBounds);
				};
			return view;
		}
	}
}