#pragma once

#include "RS485.h"
#include "FWUpdate.h"
#include "Portal.h"

#include "../Base.h"

namespace Modules {
	class Column : public Base
	{
	public:
		Column();

		string getTypeName() const override;
		string getName() const override;

		void deserialise(const nlohmann::json&) override;

		void init() override;
		void update() override;

		void populateInspector(ofxCvGui::InspectArguments& args);
		void processIncoming(const nlohmann::json&) override;

		void dragEvent(const ofDragInfo&);

		void buildPanels(size_t panelCount);

		size_t getCountX() const;
		size_t getCountY() const;

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

		ofxCvGui::PanelPtr getMiniView(float width);

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
				ofParameter<bool> flipped{ "Flipped", true };
				PARAM_DECLARE("Physical", flipped);
			} physical;

			struct : ofParameterGroup {
				ofParameter<bool> enabled{ "Enabled", true };
				ofParameter<float> period_s{ "Period [s]", 60.0f, 0.01f, 100.0f };
				PARAM_DECLARE("Scheduled poll", enabled, period_s);
			} scheduledPoll;

			PARAM_DECLARE("Column", physical, scheduledPoll);
		} parameters;

		chrono::system_clock::time_point lastPollAll = chrono::system_clock::now();

		std::string name;
	};
}