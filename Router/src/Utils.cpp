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

	//----------
	string
		millisToString(uint32_t millis)
	{
		const uint32_t ms_per_s = 1000;
		const uint32_t ms_per_m = 60 * ms_per_s;
		const uint32_t ms_per_h = 60 * ms_per_m;
		const uint32_t ms_per_d = 24 * ms_per_h;

		auto days = millis / ms_per_d;
		millis %= ms_per_d;

		auto hours = millis / ms_per_h;
		millis %= ms_per_h;

		auto minutes = millis / ms_per_m;
		millis %= ms_per_m;

		auto seconds = millis / ms_per_s;
		millis %= ms_per_s;

		stringstream ss;
		if (days > 0) {
			ss << days << "d ";
		}
		if (hours > 0) {
			ss << hours << "h ";
		}
		if (minutes > 0) {
			ss << minutes << "m ";
		}
		if (seconds > 0) {
			ss << seconds << "s ";
		}
		ss << millis << "ms";

		return ss.str();
	}
}