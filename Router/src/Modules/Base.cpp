#include "pch_App.h"
#include "Base.h"

namespace Modules {
	//----------
	shared_ptr<ofxCvGui::Widgets::SubMenuInspectable>
		Base::addSubMenuToInsecptor(shared_ptr<ofxCvGui::Panels::Widgets> inspector
			, shared_ptr<ofxCvGui::IInspectable> inspectable)
	{
		auto menuItem = inspector->addSubMenu(this->getName(), inspectable);

		// Add the glyph
		auto glyph = this->getGlyph();
		if (!glyph.empty()) {
			menuItem->onDraw += [glyph](ofxCvGui::DrawArguments& args) {
				auto bounds = args.localBounds;
				bounds.width = 30;
				bounds.x = 20;
				ofxCvGui::Utils::drawGlyph(glyph, bounds);
			};
		}

		return menuItem;
	}
}