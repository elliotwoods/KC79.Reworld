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
			ofLogError() << "Invalid axis index";
			return "";
		}
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