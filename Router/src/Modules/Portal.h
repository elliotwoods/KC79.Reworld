#pragma once

#include "Base.h"
#include "RS485.h"

namespace Modules {
	class Portal : public Base
	{
	public:
		Portal(shared_ptr<RS485>, int targetID);

		string getTypeName() const override;
		void init() override;
		void update() override;

		void populateInspector(ofxCvGui::InspectArguments&);
	protected:
		shared_ptr<RS485> rs485;

		struct : ofParameterGroup {
			ofParameter<int> targetID{ "Target ID", 1 };

			PARAM_DECLARE("Portal", targetID);
		} parameters;
	};
}