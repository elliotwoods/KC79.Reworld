#pragma once

#include "ofSerial.h"
#include "ofxCvGui.h"
#include "Base.h"
#include "Utils.h"
#include "../msgpack11/msgpack11.hpp"
#include "../SerialDevices/IDevice.h"
#include "../SerialDevices/ListedDevice.h"

namespace Modules {
	class Column;

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
			Packet(const function<msgpack11::MsgPack()>&);

			void render();

			MsgpackBinary msgpackBinary;
			bool needsACK = true;
			int32_t customWaitTime_ms = -1;
			int target = -1;
			string address;
			bool collateable = true;

			function<msgpack11::MsgPack()> lazyMessageRenderer;

			std::function<void()> onSent;
		};

		// -1 = Everybody
		// 0 = Host
		// 1-127 = Clients
		typedef int8_t Target;

		RS485(Column*);
		~RS485();

		string getTypeName() const override;
		void init() override;
		void setup(const nlohmann::json&) override;
		void update() override;
		void populateInspector(ofxCvGui::InspectArguments&);

		bool isConnected() const;

		static MsgpackBinary makeHeader(const Target&);

		void transmit(const Packet&);

		void transmitPing(const Target&);
		void transmitMessage(const Target&, const nlohmann::json&);
		void transmitHeaderAndBody(const MsgpackBinary& header
			, const MsgpackBinary& body);

		size_t getOutboxCount() const;
		void clearOutbox();
		
		void clearCounters();

		void processIncoming(const nlohmann::json&);

		vector<ofxCvGui::ElementPtr> getWidgets();

		void collateOutboxPackets();
	protected:
		Column* column;

		void openSerial(const nlohmann::json&);
		void openSerial(const SerialDevices::ListedDevice&);
		void openSerial(shared_ptr<SerialDevices::IDevice>);
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
			ofThreadChannel<nlohmann::json> inbox;
			ofThreadChannel<Packet> outbox;
		};
		shared_ptr<SerialThread> serialThread;

		struct {
			nlohmann::json settings;
			std::chrono::system_clock::time_point lastConnectionAttempt = chrono::system_clock::now();
			const std::chrono::milliseconds retryPeriod{ 20000 };
		} initilialisation;

		std::chrono::system_clock::time_point lastPoll;
		std::chrono::system_clock::time_point lastKeepAlive;
		std::chrono::system_clock::time_point lastIncomingMessageTime = std::chrono::system_clock::now();

		struct : ofParameterGroup {
			ofParameter<int> responseWindow_ms{ "Response window [ms]", 300 };
			ofParameter<int> gapBetweenBroadcastSends_ms{ "Gap between broadcast sends [ms]",  100 };
			ofParameter<int> gapAfterLastRx_ms{ "Gap after last rx [ms]",  5 };
			ofParameter<bool> collatePackets{ "Collate packets",  true };

			struct : ofParameterGroup {
				ofParameter<bool> printTx{ "Print Tx", false };
				ofParameter<bool> printRx{ "Print Rx", false };
				ofParameter<bool> printBrokenMsgpack{ "Print broken msgpack", false };
				ofParameter<bool> printACKTime{ "Print ACK time", false };
				ofParameter<bool> printMessageErrors{ "Print message errors",  false };
				ofParameter<int> targetID{ "Target ID", 1 };
				PARAM_DECLARE("Debug", printTx, printRx, printACKTime, printMessageErrors, targetID);
			} debug;
			
			PARAM_DECLARE("RS485", responseWindow_ms, gapBetweenBroadcastSends_ms, gapAfterLastRx_ms, collatePackets, debug);
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

		vector<int> repliesSeenFrom; // the ID of the sender
		ofThreadChannel<std::function<void()>> serialThreadActions;
		ofThreadChannel<std::promise<void>*> clearOutboxNotify;
	}; 
}