#pragma once

#include "ofSerial.h"
#include "ofxCvGui.h"
#include "Base.h"
#include "Utils.h"
#include "../msgpack11/msgpack11.hpp"
#include "../SerialDevices/IDevice.h"
#include "../SerialDevices/ListedDevice.h"

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
			int target = -1;

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

		void openSerial(const SerialDevices::ListedDevice&);
		void closeSerial();

		void serialThreadedFunction();
		bool serialThreadReceive();
		bool serialThreadSend();

		void updateInbox();

		struct SerialThread {
			std::thread thread;

			bool joining = false;
			shared_ptr<SerialDevices::IDevice> serialDevice;
			std::chrono::system_clock::time_point lastRxTime = chrono::system_clock::now();

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
			ofParameter<int> responseWindow_ms{ "Response window [ms]", 500 };
			ofParameter<int> gapBetweenBroadcastSends_ms{ "Gap between broadcast sends [ms]",  100 };
			ofParameter<int> gapAfterLastRx_ms{ "Gap after last rx [ms]",  5 };

			struct : ofParameterGroup {
				ofParameter<bool> printTx{ "Print Tx", false };
				ofParameter<bool> printRx{ "Print Rx", false };
				ofParameter<bool> printBrokenMsgpack{ "Print broken msgpack", false };
				ofParameter<int> targetID{ "Target ID", 1 };
				PARAM_DECLARE("Debug", printTx, printRx, targetID);
			} debug;
			
			PARAM_DECLARE("RS485", responseWindow_ms, gapBetweenBroadcastSends_ms, gapAfterLastRx_ms, debug);
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

		ofThreadChannel<int> repliesSeenFrom; // the ID of the sender
	}; 
}