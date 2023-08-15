#pragma once

#include "ofSerial.h"
#include "ofxCvGui.h"
#include "Base.h"

struct IsFrameNew {
	void notify() {
		this->notifyFrameNew = true;
	}
	void update() {
		this->isFrameNew = this->notifyFrameNew;
		this->notifyFrameNew = false;
	}
	bool isFrameNew = false;
	bool notifyFrameNew = false;
};

namespace Modules {
	class RS485 : public Base {
	public:
		struct Config {
			std::string comPort = "COM15";
		};

		// -1 = Everybody
		// 0 = Host
		// 1-127 = Clients
		typedef int8_t Target;

		RS485();

		string getTypeName() const override;
		void init() override;
		void update() override;

		void populateInspector(ofxCvGui::InspectArguments&);

		static vector<uint8_t> makeHeader(const Target&);

		void transmitRawPacket(const uint8_t* data, size_t);
		void transmitPoll(const Target&);
		void transmitMessage(const Target&, const nlohmann::json&);
		void transmitHeaderAndBody(const vector<uint8_t>& header
			, const vector<uint8_t>& body);

		void receive();

		void processIncoming(const nlohmann::json&);
	protected:
		ofSerial serial;
		std::string connectedPortName; // only valid whilst initialised

		vector<uint8_t> cobsIncoming;
		bool isFirstIncoming = true;

		std::chrono::system_clock::time_point lastPoll;
		std::chrono::system_clock::time_point lastKeepAlive;
		std::chrono::system_clock::time_point lastIncomingMessageTime = std::chrono::system_clock::now();

		struct : ofParameterGroup {
			struct : ofParameterGroup {
				ofParameter<int> targetID{ "Target ID", 1 };
				ofParameter<bool> flood{ "Flood", false };
				PARAM_DECLARE("Debug", targetID, flood);
			} debug;
			PARAM_DECLARE("RS485", debug);
		} parameters;

		struct {
			/// <summary>
			/// Used for debugging only
			/// </summary>
			vector<uint8_t> lastMessagePackTx;

			IsFrameNew isFrameNewMessageRx;
			IsFrameNew isFrameNewMessageTx;
			IsFrameNew isFrameNewMessageRxError;
			IsFrameNew isFrameNewDeviceRxSuccess;
			IsFrameNew isFrameNewDeviceRxFail;
			IsFrameNew isFrameNewDeviceError;

			size_t rxCount = 0;
			size_t txCount = 0;
		} debug;
	}; 
}