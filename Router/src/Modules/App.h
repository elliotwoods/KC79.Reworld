#pragma once

#include "Base.h"

#include "RS485.h"
#include "FWUpdate.h"
#include "Portal.h"

namespace Modules {
	class App : public Base
	{
	public:
		App();
		
		string getTypeName() const override;
		void init() override;
		void update() override;

		void populateInspector(ofxCvGui::InspectArguments& args);
		void dragEvent(const ofDragInfo&);
	protected:
		shared_ptr<RS485> rs485;
		shared_ptr<FWUpdate> fwUpdate;

		shared_ptr<Portal> portal;

		vector<shared_ptr<Base>> modules;
	};
}