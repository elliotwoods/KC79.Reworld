#pragma once

#include "ofxCvGui.h"

namespace Modules {
	typedef int32_t Steps;
	typedef int32_t StepsPerSecond;
	typedef int32_t StepsPerSecondPerSecond;

	class Base : public ofxCvGui::IInspectable
	{
	public:
		virtual string getTypeName() const = 0;
		virtual string getName() const {
			return this->getTypeName();
		}
		virtual string getGlyph() const {
			return "";
		}
		virtual void init() { };
		virtual void deserialise(const nlohmann::json&) { };
		virtual void update() { };
		
		virtual void processIncoming(const nlohmann::json&) { }

		shared_ptr<ofxCvGui::Widgets::SubMenuInspectable> addSubMenuToInsecptor(shared_ptr<ofxCvGui::Panels::Widgets> inspector
			, shared_ptr<ofxCvGui::IInspectable> inspectable);
	};
}