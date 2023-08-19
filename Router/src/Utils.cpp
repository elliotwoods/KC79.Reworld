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
		auto button = make_shared<ofxCvGui::Widgets::Button>(ofToString((int) portal->getTarget()), [portal]() {
			ofxCvGui::inspect(portal);
			});

		return button;
	}

	//----------
	IReportedState::IReportedState(const string& name)
		: name(name)
	{

	}
}