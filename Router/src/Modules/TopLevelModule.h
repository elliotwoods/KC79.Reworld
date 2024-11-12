#pragma once

#include "Base.h"

namespace Modules {
	class TopLevelModule : public Base
	{
	public:
		virtual ofxCvGui::PanelPtr getMiniView() = 0;
		virtual int getMiniViewHeight() const {
			return -1;
		}

		virtual ofxCvGui::PanelPtr getPanel() {
			return nullptr;
		}
	};
}