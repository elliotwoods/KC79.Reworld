#pragma once

#include "Base.h"
#include "RS485.h"
#include "Utils.h"

#include "PerPortal/MotorDriverSettings.h"
#include "PerPortal/Axis.h"
#include "PerPortal/Pilot.h"
#include "PerPortal/Logger.h"

#include "../msgpack11/msgpack11.hpp"

namespace Modules {
	class Portal : public Base
	{
	public:
		typedef uint8_t Target;

		struct Action {
			string caption;
			string icon;
			msgpack11::MsgPack message;
			char shortcutKey = 0;
		};

		static vector<Action> getActions();

		Portal(shared_ptr<RS485>, int Target);
		string getTypeName() const override;
		string getGlyph() const override;

		void init() override;
		void update() override;

		void poll();

		void populateInspectorPanelHeader(ofxCvGui::InspectArguments&);
		void populateInspector(ofxCvGui::InspectArguments&);
		void processIncoming(const nlohmann::json&) override;

		Target getTarget() const;
		void setTarget(Target);

		// Used by PerPortal classes to send out from module to RS485
		void sendToPortal(const msgpack11::MsgPack&);

		shared_ptr<PerPortal::MotorDriverSettings> getMotorDriverSettings();
		shared_ptr<PerPortal::Axis> getAxis(int axis);
		shared_ptr<PerPortal::Pilot> getPilot();


		ofxLiquidEvent<Target> onTargetChange;
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
				ofParameter<bool> regularly{ "Regularly", false };
				ofParameter<float> interval{ "Interval [s]", 1.0f, 0.01f, 60.0f };
				PARAM_DECLARE("Poll", regularly, interval);
			} poll;

			PARAM_DECLARE("Portal", targetID, poll);
		} parameters;

		struct {
			Utils::ReportedState<uint32_t> upTime{ "upTime" };
			Utils::ReportedState<string> version{ "version" };
			Utils::ReportedState<bool> calibrated{ "calibrated" };
			vector<Utils::IReportedState*> variables{
				&upTime
				, &version
				, & calibrated
			};
		} reportedState;

		struct {
			Utils::IsFrameNew rx;
			Utils::IsFrameNew tx;
		} isFrameNew;

		struct {
			shared_ptr<ofxCvGui::Widgets::Heartbeat> rxHeartbeat;
			shared_ptr<ofxCvGui::Widgets::Heartbeat> txHeartbeat;
		} storedWidgets; // These are not rebuilt

		chrono::system_clock::time_point lastPoll = chrono::system_clock::now();
		chrono::system_clock::time_point lastIncoming = chrono::system_clock::now();
	};
}