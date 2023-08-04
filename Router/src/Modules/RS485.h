#pragma once

#include "ofSerial.h"
#include "ofxCvGui.h"

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
	class RS485 : public ofxCvGui::IInspectable {
	public:
		struct Config {
			std::string comPort = "COM15";
		};

		// 0 = Host
		// -1 = Everybody
		typedef int8_t Target;

		RS485();
		void init();
		void update();

		ofxCvGui::PanelPtr getPanel();
		void populateInspector(ofxCvGui::InspectArguments&);

		static vector<uint8_t> makeHeader(const Target&);

		void transmitPoll(const Target&);
		void transmitMessage(const Target&, const nlohmann::json&);
		void transmitHeaderAndBody(const vector<uint8_t>& header
			, const vector<uint8_t>& body);

		void receive();

		void processIncoming(const nlohmann::json&);

		void sendTestMessage();
	protected:
		ofSerial serial;
		std::string connectedPortName; // only valid whilst initialised

		vector<uint8_t> cobsIncoming;
		bool isFirstIncoming = true;

		std::chrono::system_clock::time_point lastPoll;
		std::chrono::system_clock::time_point lastKeepAlive;
		std::chrono::system_clock::time_point lastIncomingMessageTime = std::chrono::system_clock::now();

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