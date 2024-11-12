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
			this->onPopulateInspector += [this](ofxCvGui::InspectArguments& args) {
				this->populateInspector(args);
				};
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
					this->isFrameNew.notify();
				}
			}

			this->isFrameNew.update();
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
			auto view = ofxCvGui::Panels::makeWidgets();
			{
				auto stack = view->addHorizontalStack();
				{
					{
						auto element = make_shared<ofxCvGui::Widgets::Indicator>("Running", [this]() {
							if ((bool)this->oscReceiver) {
								return ofxCvGui::Widgets::Indicator::Status::Good;
							}
							else {
								return ofxCvGui::Widgets::Indicator::Status::Clear;
							}
							});
						stack->add(element);
					}
					{
						auto element = make_shared<ofxCvGui::Widgets::LiveValue<int>>("Port", [this]() {
							return this->parameters.port.get();
							});
						stack->add(element);
					}
				}
			}

			{
				view->addLiveValueHistory("Messages", [this]() {
					return (float)this->isFrameNew.eventCount;
					});
			}

			return view;
		}

		//----------
		int
			Receiver::getMiniViewHeight() const
		{
			return 120;
		}
	}
}