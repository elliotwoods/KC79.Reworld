#pragma once

#include "ofxCvGui.h"

namespace Modules {
	typedef int32_t Steps;

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
		virtual void update() = 0;
		
		virtual void processIncoming(const nlohmann::json&) {
		}

		void addSubMenuToInsecptor(shared_ptr<ofxCvGui::Panels::Inspector>, shared_ptr<IInspectable>);
	};
}