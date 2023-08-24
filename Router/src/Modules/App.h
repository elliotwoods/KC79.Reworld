#pragma once

#include "Base.h"

#include "RS485.h"
#include "FWUpdate.h"
#include "Portal.h"
#include "crow/crow.h"

namespace Modules {
	class App : public Base
	{
	public:
		App();
		~App();
		
		string getTypeName() const override;
		void init() override;
		void update() override;

		void populateInspector(ofxCvGui::InspectArguments& args);
		void processIncoming(const nlohmann::json&) override;

		void dragEvent(const ofDragInfo&);

		void buildPanels(size_t panelCount);
		shared_ptr<Portal> getPortalByTargetID(Portal::Target);

		void broadcastInit();
		void broadcastCalibrate();
		void broadcastFlashLED();
		void broadcastHome();
		void broadcastSeeThrough();
		void broadcastReset();
		void broadcast(const msgpack11::MsgPack&);

	protected:
		void refreshPortalsByID();
		void setupCrowRoutes();

		shared_ptr<RS485> rs485;
		shared_ptr<FWUpdate> fwUpdate;

		vector<shared_ptr<Portal>> portals;

		map<Portal::Target, shared_ptr<Portal>> portalsByID;
		bool portalsByIDDirty = true;

		vector<shared_ptr<Base>> modules;

		crow::SimpleApp crow;
		std::future<void> crowRun;
	};
}