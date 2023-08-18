#pragma once

#include "ofSerial.h"
#include "ofxCvGui.h"
#include "Base.h"
#include "../msgpack11/msgpack11.hpp"

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
	class App;

	class RS485 : public Base {
	public:
		struct Config {
			std::string comPort = "COM15";
		};

		// A messagepack encoded message (not COBS yet)
		typedef vector<uint8_t> MsgpackBinary;

		// -1 = Everybody
		// 0 = Host
		// 1-127 = Clients
		typedef int8_t Target;

		RS485(App *);

		string getTypeName() const override;
		void init() override;
		void update() override;
		void populateInspector(ofxCvGui::InspectArguments&);

		static MsgpackBinary makeHeader(const Target&);

		void transmit(const msgpack11::MsgPack&);
		void transmit(const msgpack_sbuffer&);
		void transmit(const MsgpackBinary& packetContent);

		void transmitPing(const Target&);
		void transmitMessage(const Target&, const nlohmann::json&);
		void transmitHeaderAndBody(const MsgpackBinary& header
			, const MsgpackBinary& body);

		void processIncoming(const nlohmann::json&);
	protected:
		App * app;

		void openSerial(int deviceID);
		void closeSerial();

		void serialThreadedFunction();
		bool serialThreadReceive();
		bool serialThreadSend();

		void updateInbox();

		struct SerialThread {
			std::thread thread;

			bool joining = false;
			ofSerial serial;
			vector<uint8_t> cobsIncoming;
			bool isFirstIncoming = true;
			ofThreadChannel<MsgpackBinary> inbox;
			ofThreadChannel<MsgpackBinary> outbox;
		};
		shared_ptr<SerialThread> serialThread;

		std::string connectedPortName; // only valid whilst initialised

		std::chrono::system_clock::time_point lastPoll;
		std::chrono::system_clock::time_point lastKeepAlive;
		std::chrono::system_clock::time_point lastIncomingMessageTime = std::chrono::system_clock::now();

		struct : ofParameterGroup {
			ofParameter<int> baudRate{ "Baud rate", 115200 };
			struct : ofParameterGroup {
				ofParameter<int> targetID{ "Target ID", 1 };
				PARAM_DECLARE("Debug", targetID);
			} debug;
			PARAM_DECLARE("RS485", baudRate, debug);
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