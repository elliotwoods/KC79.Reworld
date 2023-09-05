#pragma once

#include "RS485.h"
#include "FWUpdate.h"
#include "Portal.h"

#include "Base.h"

namespace Modules {
	class Column : public Base
	{
	public:
		Column(const nlohmann::json&);

		string getTypeName() const override;
		string getName() const override;

		void init() override;
		void update() override;

		void populateInspector(ofxCvGui::InspectArguments& args);
		void processIncoming(const nlohmann::json&) override;

		void dragEvent(const ofDragInfo&);

		void buildPanels(size_t panelCount);

		vector<shared_ptr<Portal>> getAllPortals() const;
		shared_ptr<Portal> getPortalByTargetID(Portal::Target);

		shared_ptr<RS485> getRS485();
		shared_ptr<FWUpdate> getFWUpdate();

		void pollAll();

		void broadcast(const msgpack11::MsgPack&);
		void broadcastInit();
		void broadcastCalibrate();
		void broadcastFlashLED();
		void broadcastHome();
		void broadcastSeeThrough();
		void broadcastEscapeFromRoutine();
		void broadcastReset();

	protected:
		void refreshPortalsByID();

		shared_ptr<RS485> rs485;
		shared_ptr<FWUpdate> fwUpdate;

		vector<shared_ptr<Portal>> portals;

		map<Portal::Target, shared_ptr<Portal>> portalsByID;
		bool portalsByIDDirty = true;

		vector<shared_ptr<Base>> modules;

		struct : ofParameterGroup {
			struct : ofParameterGroup {
				ofParameter<bool> enabled{ "Enabled", false };
				ofParameter<float> period_s{ "Period [s]", 5.0f, 0.01f, 100.0f };
				PARAM_DECLARE("Scheduled poll", enabled, period_s);
			} scheduledPoll;
			PARAM_DECLARE("App", scheduledPoll);
		} parameters;

		chrono::system_clock::time_point lastPollAll = chrono::system_clock::now();

		std::string name;
	};
}