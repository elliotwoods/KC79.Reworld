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
		virtual void init() = 0;
		virtual void setup(const nlohmann::json&) {};
		virtual void update() = 0;
		
		virtual void processIncoming(const nlohmann::json&) {
		}

		void addSubMenuToInsecptor(shared_ptr<ofxCvGui::Panels::Inspector>, shared_ptr<IInspectable>);
	};
}