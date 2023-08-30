#include "pch_App.h"
#include "RS485.h"
#include "App.h"
#include "../SerialDevices/listDevices.h"

#include "../cobs-c/cobs.h"
#include "msgpack.h"

// For debugging
void printChar(int byte) {
	cout << "0x" << std::hex << byte;
	if (byte >= 32 && byte < 127) {
		cout << " '" << (char)byte << "'";
	}
	cout << ", ";
};

namespace Modules {
#pragma mark Packet
	//----------
	RS485::Packet::Packet()
	{

	}

	//----------
	RS485::Packet::Packet(const MsgpackBinary& msgpackBinary)
		: msgpackBinary(msgpackBinary)
	{

	}

	//----------
	RS485::Packet::Packet(const msgpack11::MsgPack& message)
	{
		auto dataString = message.dump();
		auto dataBegin = (uint8_t*)dataString.data();
		auto dataEnd = dataBegin + dataString.size();
		this->msgpackBinary.assign(dataBegin, dataEnd);
	}

	//----------
	RS485::Packet::Packet(const msgpack_sbuffer& buffer)
	{
		auto data = (uint8_t*)buffer.data;
		this->msgpackBinary.assign(data, data + buffer.size);
	}

#pragma mark RS485
	//----------
	RS485::RS485(App * app)
		: app(app)
	{

	}

	//----------
	RS485::~RS485()
	{
		this->closeSerial();
	}

	//----------
	string
		RS485::getTypeName() const
	{
		return "RS485";
	}

	//----------
	void
		RS485::init()
	{
		this->onPopulateInspector += [this](ofxCvGui::InspectArguments& args) {
			this->populateInspector(args);
		};
	}

	//----------
	void
		RS485::update()
	{
		this->updateInbox();

		{
			this->debug.isFrameNewMessageRx.update();
			this->debug.isFrameNewMessageTx.update();
			this->debug.isFrameNewMessageRxError.update();
			this->debug.isFrameNewDeviceRxSuccess.update();
			this->debug.isFrameNewDeviceRxFail.update();
			this->debug.isFrameNewDeviceError.update();
		}
	}

	//----------
	void
		RS485::populateInspector(ofxCvGui::InspectArguments& args)
	{
		auto inspector = args.inspector;

		inspector->addTitle("Device");
		inspector->addIndicatorBool("Connected", [this]() {
			return (bool) this->serialThread;
			});

		// If we don't have a device, create a list of options
		if (!this->serialThread) {

			inspector->addTitle("Connect to", ofxCvGui::Widgets::Title::Level::H2);

			auto devices = SerialDevices::listDevices();

			string currentDeviceType = "";
			for (auto device : devices) {
				if (device.type != currentDeviceType) {
					inspector->addTitle(device.type, ofxCvGui::Widgets::Title::Level::H3);
					currentDeviceType = device.type;
				}

				inspector->addButton(device.name + "\n" + device.address, [this, device]() {
					this->openSerial(device);
					ofxCvGui::refreshInspector(this);
					})->setHeight(70.0f);
			}
		}
		else {
			inspector->addLiveValue<string>("Device name", [this]() {
				return this->connectedPortName;
				});
			inspector->addButton("Disconnect", [this]() {
				this->closeSerial();
				ofxCvGui::refreshInspector(this);
				});
		}

		inspector->addSpacer();

		if (this->serialThread) {
			inspector->addButton("Ping", [this]() {
				this->transmitPing((Target) this->parameters.debug.targetID.get());
				}, ' ');
		}

		{
			auto widget = make_shared<ofxCvGui::Widgets::Heartbeat>("Rx", [this]() {
				return this->debug.isFrameNewMessageRx.isFrameNew;
				});
			inspector->add(widget);
		}

		inspector->addLiveValue<size_t>("Rx count", [this]() {
			return this->debug.rxCount;
			});

		{
			auto widget = make_shared<ofxCvGui::Widgets::Heartbeat>("Tx", [this]() {
				return this->debug.isFrameNewMessageTx.isFrameNew;
				});
			inspector->add(widget);
		}

		inspector->addLiveValue<size_t>("Tx count", [this]() {
			return this->debug.txCount;
			});

		inspector->addLiveValue<size_t>("Outbox count", [this]() {
			if (this->serialThread) {
				return this->serialThread->outbox.size();
			}
			else {
				return (size_t)0;
			}
			});

		{
			auto widget = make_shared<ofxCvGui::Widgets::Heartbeat>("Rx Error", [this]() {
				return this->debug.isFrameNewMessageRxError.isFrameNew;
				});
			inspector->add(widget);
		}

		{
			auto widget = make_shared<ofxCvGui::Widgets::Heartbeat>("Device Rx OK", [this]() {
				return this->debug.isFrameNewDeviceRxSuccess.isFrameNew;
				});
			inspector->add(widget);
		}

		{
			auto widget = make_shared<ofxCvGui::Widgets::Heartbeat>("Device Rx Fail", [this]() {
				return this->debug.isFrameNewDeviceRxFail.isFrameNew;
				});
			inspector->add(widget);
		}

		{
			auto widget = make_shared<ofxCvGui::Widgets::Heartbeat>("Device Error", [this]() {
				return this->debug.isFrameNewDeviceError.isFrameNew;
				});
			inspector->add(widget);
		}

		inspector->addSpacer();

		inspector->addParameterGroup(this->parameters);
	}

	//----------
	bool
		RS485::isConnected() const
	{
		// if there's no thread then not connected
		return (bool)this->serialThread;
	}

	//----------
	RS485::MsgpackBinary
		RS485::makeHeader(const Target& target)
	{
		msgpack_sbuffer headerBuffer;
		msgpack_packer packer;
		msgpack_sbuffer_init(&headerBuffer);
		msgpack_packer_init(&packer
			, &headerBuffer
			, msgpack_sbuffer_write);

		msgpack_pack_array(&packer, 3);
		{
			// First element is target address
			msgpack_pack_fix_int8(&packer, target);

			// Second element is source address
			msgpack_pack_fix_int8(&packer, 0);

			// Third element will be the message body
		}

		// Convert to vector<uint8_t>
		MsgpackBinary header;
		header.assign(headerBuffer.data, headerBuffer.data + headerBuffer.size);
		msgpack_sbuffer_destroy(&headerBuffer);

		return header;
	}

	//----------
	void
		RS485::transmit(const Packet& packet)
	{
		if (!this->isConnected()) {
			ofLogError() << "Cannot transmit when serial is closed";
			return;
		}

		this->serialThread->outbox.send(packet);
	}

	//----------
	void
		RS485::transmitPing(const Target& target)
	{
		this->transmit(Packet(
			msgpack11::MsgPack::array{
				(int8_t)target
				, (int8_t)0
				, msgpack11::MsgPack()
				}
			));
		}

	//----------
	void
		RS485::transmitMessage(const Target& target
			, const nlohmann::json& jsonMessage)
	{
		nlohmann::json txJson;
		txJson[target] = jsonMessage;

		// Create the message header
		auto header = RS485::makeHeader(target);

		// Create messagepack body from json
		auto bodyMsgback = nlohmann::json::to_msgpack(txJson);

		this->transmitHeaderAndBody(header, bodyMsgback);
	}

	//----------
	void
		RS485::transmitHeaderAndBody(const MsgpackBinary& header
			, const MsgpackBinary& body)
	{
		// Combine header and body
		MsgpackBinary headerAndBody = header;
		headerAndBody.insert(headerAndBody.end()
			, body.begin()
			, body.end());

		this->transmit(headerAndBody);
	}

	//----------
	void
		RS485::processIncoming(const nlohmann::json& json)
	{
		if (this->parameters.debug.printRx.get()) {
			cout << "Rx : " << json.dump(4) << endl;
		}

		this->app->processIncoming(json);
	}

	//----------
	void
		RS485::openSerial(const SerialDevices::ListedDevice& listedDevice)
	{
		if (this->serialThread) {
			this->closeSerial();
		}

		auto serialDevice = listedDevice.createDevice();

		if (!serialDevice) {
			ofLogError() << "Could not open serial device " + listedDevice.type + "::" + listedDevice.name;
			return;
		}

		auto serialThread = make_shared<SerialThread>();

		serialThread->serialDevice = serialDevice;

		serialThread->thread = std::thread([this]() {
			this->serialThreadedFunction();
			});

		this->serialThread = serialThread;
	}

	//----------
	void
		RS485::closeSerial()
	{
		if (!this->serialThread) {
			return;
		}

		this->serialThread->joining = true;
		this->serialThread->inbox.close();
		this->serialThread->outbox.close();
		this->serialThread->thread.join();
		this->serialThread->serialDevice->close();
		this->serialThread.reset();
	}

	//----------
	void
		RS485::serialThreadedFunction()
	{
		while (!this->serialThread->joining) {
			auto didRx = this->serialThreadReceive();
			if (didRx) {
				this->serialThread->lastRxTime = chrono::system_clock::now();
			}

			bool didTx = false;
			
			// check deadtime between last rx before we allow next tx
			if (!didRx) {
				auto timeSinceLastRx = chrono::system_clock::now() - this->serialThread->lastRxTime;
				if (timeSinceLastRx > chrono::milliseconds(this->parameters.gapAfterLastRx_ms.get())) {
					didTx = this->serialThreadSend();
				}
			}

			// Sleep if we didn't do any work this loop
			if (!didRx || didTx) {
				std::this_thread::sleep_for(std::chrono::milliseconds(1));
			}
		}

	}

	//-----------
	bool
		RS485::serialThreadReceive()
	{
		auto rxData = this->serialThread->serialDevice->receiveBytes();

		if (rxData.empty()) {
			return false;
		}

		for(auto word : rxData) {
			// If end of COB packet
			if (word == 0) {
				// If there's nothing to decode, don't do anything
				if (this->serialThread->cobsIncoming.empty()) {
					continue;
				}

				// Clear incoming buffer (and keep a copy for use here)
				auto copyOfIncoming = this->serialThread->cobsIncoming;
				this->serialThread->cobsIncoming.clear();

				// Decode the COBS message
				vector<uint8_t> binaryMessagePack(copyOfIncoming.size());
				auto decodeResult = cobs_decode(binaryMessagePack.data(),
					binaryMessagePack.size(),
					copyOfIncoming.data(),
					copyOfIncoming.size());

				// Check if decoded OK (should predictably be true)
				if (decodeResult.status != COBS_DECODE_OK) {
					ofLogError("LaserSystem") << "COBS decode error " << (int)decodeResult.status;
					this->debug.isFrameNewDeviceRxFail.notify();
					continue;
				}
				binaryMessagePack.resize(decodeResult.out_len);

				if (this->parameters.debug.printRx.get()) {
					cout << "Rx : ";
					for (const auto& byte : binaryMessagePack) {
						printChar(byte);
					}
					cout << endl;
				}

				this->serialThread->inbox.send(binaryMessagePack);
			}
			// Continuation of COB packet
			else {
				this->serialThread->cobsIncoming.push_back(word);
			}
		}

		return true;
	}

	//-----------
	bool
		RS485::serialThreadSend()
	{
		if (this->serialThread->outbox.size() == 0) {
			return false;
		}

		Packet packet;
		while (this->serialThread->outbox.tryReceive(packet)) {
			const auto& msgpackBinary = packet.msgpackBinary;

			auto data = msgpackBinary.data();
			auto size = msgpackBinary.size();

			// allocate buffer of max size for cobs
			vector<uint8_t> binaryCOBS(size * 255 / 254 + 2);

			// Perform the encode
			auto encodeResult = cobs_encode(binaryCOBS.data()
				, binaryCOBS.size()
				, data
				, size);

			// Check we encoded OK
			if (encodeResult.status != COBS_ENCODE_OK) {
				ofLogError("RS485") << "Failed to encode COBS : " << encodeResult.status;
				continue;
			}

			// Crop the message to the correct number of bytes
			binaryCOBS.resize(encodeResult.out_len);

			// add a zero on the end
			binaryCOBS.push_back(0);

			// Clear the incoming ACKs
			{
				int _;
				while (this->repliesSeenFrom.tryReceive(_)) {}
			}

			// Send the data to serial
			auto bytesWritten = this->serialThread->serialDevice->transmit(binaryCOBS);

			{
				if (this->parameters.debug.printTx.get()) {
					{
						cout << "Tx COBS : ";
						for (const auto& byte : binaryCOBS) {
							printChar(byte);
						}
						cout << endl;
					}

					{
						cout << "Tx msgpack : ";
						for (const auto& byte : msgpackBinary) {
							printChar(byte);
						}
						cout << endl;
					}
				}
			}

			// Check that all bytes were sent
			if (bytesWritten != binaryCOBS.size()) {
				ofLogError("RS485") << "Failed to write all " << binaryCOBS.size() << " bytes to stream";
			}

			// Store the messagepack representation in case the user wants to debug it
			this->debug.lastMessagePackTx.assign(data, data + size);

			this->debug.isFrameNewMessageTx.notify();
			this->debug.txCount++;

			// Notify any listeners
			{
				if (packet.onSent) {
					packet.onSent();
				}
			}

			// Function to wait to receive an ACK
			auto waitForReceive = [this](int senderID, std::chrono::milliseconds& duration) {
				auto responseWindowEnd = chrono::system_clock::now() + duration;
				while (chrono::system_clock::now() < responseWindowEnd) {
					this->serialThreadReceive();

					// see if we got a message
					{
						int receivedMessageFrom;
						bool seen = false;
						while (this->repliesSeenFrom.tryReceive(receivedMessageFrom)) {
							if (receivedMessageFrom == senderID) {
								return true;
							}
						}

						this_thread::sleep_for(chrono::milliseconds(1));
					}
				}

				return false;
			};

			// After we send, we try to receive for up to the duration of the response window
			if (packet.needsACK) {
				auto waitDuration = packet.customWaitTime_ms > 0
					? chrono::milliseconds(packet.customWaitTime_ms)
					: chrono::milliseconds(this->parameters.responseWindow_ms.get());

				if (!waitForReceive(packet.target, waitDuration)) {
					ofLogError() << "ACK not seen from " << packet.target;
				}
				cout << "normal ACK" << endl;
			}
			else {
				// This is likely a broadcast packet, and we need to wait after sending
				this_thread::sleep_for(chrono::milliseconds(this->parameters.gapBetweenBroadcastSends_ms.get()));
					cout << "broadcast ACK" << endl;
			}
		}

		return true;
	}

	//----------
	void
		RS485::updateInbox()
	{
		if (!this->serialThread) {
			return;
		}

		MsgpackBinary msgpackBinary;

		while (this->serialThread->inbox.tryReceive(msgpackBinary)) {
			// Decode messagepack
			nlohmann::json json;
			try {
				json = nlohmann::json::from_msgpack(msgpackBinary);
			}
			catch (const std::exception& e) {
				ofLogError("LaserSystem") << "msgpack deserialize error : " << e.what();

				if (this->parameters.debug.printBrokenMsgpack.get()) {
					cout << "msgpack : ";
					for (const auto& byte : msgpackBinary) {
						printChar(byte);
					}
					cout << endl;
				}

				this->debug.isFrameNewMessageRxError.notify();

				continue;
			}

			// Perform deserialize on json
			try {
				this->processIncoming(json);
			}
			catch (std::exception& e) {
				ofLogError() << "Process incoming error" << e.what();
			}
			catch (const nlohmann::detail::type_error& e) {
				ofLogError() << "Incoming JSON error" << e.what();
			}
			catch (...) {
				ofLogError() << "Process incoming error";
			}
			this->lastIncomingMessageTime = std::chrono::system_clock::now();
			this->debug.isFrameNewMessageRx.notify();
			this->debug.rxCount++;
		}
	}
}