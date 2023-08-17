#pragma once

#include "../Base.h"
#include "Contants.h"

namespace Modules {
	class Portal;
	namespace PerPortal {
		class MotorDriver : public Base
		{
		public:
			MotorDriver(Portal *, int axisIndex);

			string getTypeName() const override;
			string getGlyph() const override;

			void init() override;
			void update() override;
			void populateInspector(ofxCvGui::InspectArguments&);
		protected:

			const Portal * portal;
			const int axisIndex;
		};
	}
}