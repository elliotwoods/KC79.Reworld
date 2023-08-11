#pragma once

#include "ofxCvGui.h"

namespace Modules {
	class Base : public ofxCvGui::IInspectable
	{
	public:
		virtual string getTypeName() const = 0;
		virtual void init() = 0;
		virtual void update() = 0;
	};
}