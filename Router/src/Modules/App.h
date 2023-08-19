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
		void processIncoming(const nlohmann::json&) override;

		void dragEvent(const ofDragInfo&);
	protected:
		void refreshPortalsByID();

		shared_ptr<RS485> rs485;
		shared_ptr<FWUpdate> fwUpdate;

		vector<shared_ptr<Portal>> portals;

		map<uint8_t, shared_ptr<Portal>> portalsByID;
		bool portalsByIDDirty = true;

		vector<shared_ptr<Base>> modules;
	};
}