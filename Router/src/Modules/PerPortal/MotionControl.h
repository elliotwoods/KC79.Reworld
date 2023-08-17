#pragma once

#include "../Base.h"

namespace Modules {
	class Portal;
	
	typedef int32_t Steps;

	namespace PerPortal {
		class MotionControl : public Base
		{
		public:
			MotionControl(Portal *, int axisIndex);
			string getTypeName() const override;
			string getGlyph() const override;

			string getName() const override;
			string getFWModuleName() const;

			void init();
			void update();
			void populateInspector(ofxCvGui::InspectArguments& args);

			void move(Steps position);

		protected:
			Portal* portal;
			const int axisIndex;
		};
	}
}