#pragma once

#include "Base.h"
#include "RS485.h"

#include "PerPortal/MotorDriverSettings.h"
#include "PerPortal/Axis.h"
#include "PerPortal/Pilot.h"
#include "PerPortal/Logger.h"

#include "../msgpack11/msgpack11.hpp"

namespace Modules {
	class Portal : public Base
	{
	public:
		Portal(shared_ptr<RS485>, int targetID);
		string getTypeName() const override;
		string getGlyph() const override;

		void init() override;
		void update() override;

		void populateInspector(ofxCvGui::InspectArguments&);
		void processIncoming(const nlohmann::json&) override;

		void poll();
		void initRoutine();

		void sendToPortal(const msgpack11::MsgPack&);

		shared_ptr<PerPortal::MotorDriverSettings> getMotorDriverSettings();
		shared_ptr<PerPortal::Axis> getAxis(int axis);
	protected:
		shared_ptr<RS485> rs485;
		
		shared_ptr<PerPortal::MotorDriverSettings> motorDriverSettings;
		shared_ptr<PerPortal::Axis> axis[2];
		shared_ptr<PerPortal::Pilot> pilot;
		shared_ptr<PerPortal::Logger> logger;

		vector<shared_ptr<Base>> submodules;

		struct : ofParameterGroup {
			ofParameter<int> targetID{ "Target ID", 1 };

			struct : ofParameterGroup {
				ofParameter<bool> regularly{ "Regularly", true };
				ofParameter<float> interval{ "Interval [s]", 1.0f };
				PARAM_DECLARE("Poll", regularly, interval);
			} poll;

			PARAM_DECLARE("Portal", targetID, poll);
		} parameters;

		chrono::system_clock::time_point lastPoll = chrono::system_clock::now();
	};
}