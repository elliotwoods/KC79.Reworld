#include "pch_App.h"

#include "Utils.h"
#include "Modules/Portal.h"

namespace Utils{
	//----------
	std::string
		getAxisLetter(int axisIndex)
	{
		switch (axisIndex) {
		case 0:
			return "A";
		case 1:
			return "B";
		default:
			ofLogError("Invalid axis index");
			return "";
		}
	}

	//----------
	shared_ptr<ofxCvGui::Widgets::Button>
		makeButton(shared_ptr<Modules::Portal> portal)
	{
		auto action = [portal]() {
			ofxCvGui::inspect(portal);
		};
		auto numberString = ofToString((int)portal->getTarget());
		auto button = (int)portal->getTarget() < 10
			? make_shared<ofxCvGui::Widgets::Button>(numberString, action, numberString[0])
			: make_shared<ofxCvGui::Widgets::Button>(numberString, action);
		
		return button;
	}

	//----------
	IReportedState::IReportedState(const string& name)
		: name(name)
	{

	}

	//----------
	ofxCvGui::ElementPtr
		makeGUIElementTyped(ReportedState<bool>* variable)
	{
		return make_shared<ofxCvGui::Widgets::Indicator>(variable->name, [variable]() {
			if (variable->value) {
				return ofxCvGui::Widgets::Indicator::Status::Good;
			}
			else {
				return ofxCvGui::Widgets::Indicator::Status::Clear;
			}
			});
	}

	//----------
	ofxCvGui::ElementPtr
		makeGUIElementTyped(ReportedState<string>* variable)
	{
		return make_shared<ofxCvGui::Widgets::LiveValue<string>>(variable->name, [variable]() {
			return variable->value;
			});
	}

	//----------
	ofxCvGui::ElementPtr
		makeGUIElementTyped(ReportedState<float>* variable)
	{
		return make_shared<ofxCvGui::Widgets::LiveValue<float>>(variable->name, [variable]() {
			return variable->value;
			});
	}

	//----------
	ofxCvGui::ElementPtr
		makeGUIElementTyped(ReportedState<double>* variable)
	{
		return make_shared<ofxCvGui::Widgets::LiveValue<float>>(variable->name, [variable]() {
			return variable->value;
			});
	}

	//----------
	ofxCvGui::ElementPtr
		makeGUIElement(IReportedState* variable)
	{
		return make_shared<ofxCvGui::Widgets::LiveValue<string>>(variable->name
			, [variable]() {
				return variable->toString();
			});

		// ignore the following for now:
		{ auto castVariable = dynamic_cast<ReportedState<int8_t>*>(variable); if (castVariable) return makeGUIElementTyped(castVariable); }
		{ auto castVariable = dynamic_cast<ReportedState<int16_t>*>(variable); if (castVariable) return makeGUIElementTyped(castVariable); }
		{ auto castVariable = dynamic_cast<ReportedState<int32_t>*>(variable); if (castVariable) return makeGUIElementTyped(castVariable); }
		{ auto castVariable = dynamic_cast<ReportedState<uint8_t>*>(variable); if (castVariable) return makeGUIElementTyped(castVariable); }
		{ auto castVariable = dynamic_cast<ReportedState<uint16_t>*>(variable); if (castVariable) return makeGUIElementTyped(castVariable); }
		{ auto castVariable = dynamic_cast<ReportedState<uint32_t>*>(variable); if (castVariable) return makeGUIElementTyped(castVariable); }
		{ auto castVariable = dynamic_cast<ReportedState<float>*>(variable); if (castVariable) return makeGUIElementTyped(castVariable); }
		{ auto castVariable = dynamic_cast<ReportedState<double>*>(variable); if (castVariable) return makeGUIElementTyped(castVariable); }
		{ auto castVariable = dynamic_cast<ReportedState<bool>*>(variable); if (castVariable) return makeGUIElementTyped(castVariable); }
		{ auto castVariable = dynamic_cast<ReportedState<string>*>(variable); if (castVariable) return makeGUIElementTyped(castVariable); }
	}
}