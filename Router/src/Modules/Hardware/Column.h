#pragma once

#include "RS485.h"
#include "FWUpdate.h"
#include "Portal.h"

#include "../Base.h"

namespace Modules {
	class Column : public Base
	{
	public:
		struct Settings {
			size_t index;

			size_t countX;
			size_t countY;
			bool flipped;
		};

		Column(const Settings&);

		string getTypeName() const override;
		string getName() const override;

		void deserialise(const nlohmann::json&) override;

		void init() override;
		void update() override;

		void populateInspector(ofxCvGui::InspectArguments& args);
		void processIncoming(const nlohmann::json&) override;

		void rebuildPortals();

		/// <summary>
		/// In the case of Reworld Type 1, each Columns is itself 3 Portals wide
		/// </summary>
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
		vector<shared_ptr<Base>> submodules;

		size_t columnIndex = 0;
		size_t countX = 1;
		size_t countY = 1;
		vector<shared_ptr<Portal>> portals;

		map<Portal::Target, shared_ptr<Portal>> portalsByID;
		bool portalsByIDDirty = true;


		struct : ofParameterGroup {
			struct : ofParameterGroup {
				ofParameter<size_t> countX{ "Count X", 1 };
				ofParameter<size_t> countY{ "Count Y", 1 };
				ofParameter<bool> flipped{ "Flipped", true };
				PARAM_DECLARE("Arrangement", countX, countY, flipped);
			} arrangement;

			struct : ofParameterGroup {
				ofParameter<bool> enabled{ "Enabled", true };
				ofParameter<float> period_s{ "Period [s]", 60.0f, 0.01f, 100.0f };
				PARAM_DECLARE("Scheduled poll", enabled, period_s);
			} scheduledPoll;

			PARAM_DECLARE("Column", arrangement, scheduledPoll);
		} parameters;

		chrono::system_clock::time_point lastPollAll = chrono::system_clock::now();

		std::string name;
	};
}