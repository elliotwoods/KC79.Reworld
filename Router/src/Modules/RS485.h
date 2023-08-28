#pragma once

#include "ofSerial.h"
#include "ofxCvGui.h"
#include "Base.h"
#include "Utils.h"
#include "../msgpack11/msgpack11.hpp"

namespace Modules {
	class App;

	class RS485 : public Base {
	public:
		struct Config {
			std::string comPort = "COM15";
		};

		// A messagepack encoded message (not COBS yet)
		typedef vector<uint8_t> MsgpackBinary;

		struct Packet {
			Packet();
			Packet(const MsgpackBinary&);
			Packet(const msgpack11::MsgPack&);
			Packet(const msgpack_sbuffer&);

			MsgpackBinary msgpackBinary;
			bool needsACK = true;
			int32_t customWaitTime_ms = -1;

			std::function<void()> onSent;
		};

		// -1 = Everybody
		// 0 = Host
		// 1-127 = Clients
		typedef int8_t Target;

		RS485(App*);
		~RS485();

		string getTypeName() const override;
		void init() override;
		void update() override;
		void populateInspector(ofxCvGui::InspectArguments&);

		bool isConnected() const;

		static MsgpackBinary makeHeader(const Target&);

		void transmit(const Packet&);

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
			ofThreadChannel<Packet> outbox;
		};
		shared_ptr<SerialThread> serialThread;

		std::string connectedPortName; // only valid whilst initialised

		std::chrono::system_clock::time_point lastPoll;
		std::chrono::system_clock::time_point lastKeepAlive;
		std::chrono::system_clock::time_point lastIncomingMessageTime = std::chrono::system_clock::now();

		struct : ofParameterGroup {
			ofParameter<int> baudRate{ "Baud rate", 115200 };
			ofParameter<int> responseWindow_ms{ "Response window [ms]", 500 };
			ofParameter<int> gapBetweenBroadcastSends_ms{ "Gap between broadcast sends [ms]",  100 };

			struct : ofParameterGroup {
				ofParameter<bool> printResponses{ "Print responses", false };
				ofParameter<int> targetID{ "Target ID", 1 };
				PARAM_DECLARE("Debug", printResponses, targetID);
			} debug;

			
			PARAM_DECLARE("RS485", baudRate, responseWindow_ms, gapBetweenBroadcastSends_ms, debug);
		} parameters;

		struct {
			/// <summary>
			/// Used for debugging only
			/// </summary>
			vector<uint8_t> lastMessagePackTx;

			Utils::IsFrameNew isFrameNewMessageRx;
			Utils::IsFrameNew isFrameNewMessageTx;
			Utils::IsFrameNew isFrameNewMessageRxError;
			Utils::IsFrameNew isFrameNewDeviceRxSuccess;
			Utils::IsFrameNew isFrameNewDeviceRxFail;
			Utils::IsFrameNew isFrameNewDeviceError;

			size_t rxCount = 0;
			size_t txCount = 0;
		} debug;
	}; 
}