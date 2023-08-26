#include "pch_App.h"
#include "RS485.h"
#include "App.h"

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
		if (!this->serialThread) {

			inspector->addTitle("Connect to", ofxCvGui::Widgets::Title::Level::H2);

			ofSerial serialEnumerator;
			auto devices = serialEnumerator.getDeviceList();
			for (auto device : devices) {
				auto deviceName = device.getDeviceName();
				auto deviceID = device.getDeviceID();
				inspector->addButton(deviceName, [this, deviceID]() {
					this->openSerial(deviceID);
					ofxCvGui::refreshInspector(this);
					});
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
		RS485::transmit(const msgpack11::MsgPack& message, bool needsACK)
	{
		auto dataString = message.dump();
		auto dataBegin = (uint8_t*)dataString.data();
		auto dataEnd = dataBegin + dataString.size();
		auto data = vector<uint8_t>(dataBegin, dataEnd);
		this->transmit(data, needsACK);
	}

	//----------
	void
		RS485::transmit(const msgpack_sbuffer& buffer, bool needsACK)
	{
		auto data = (uint8_t*) buffer.data;
		auto message = MsgpackBinary(data, data + buffer.size);
		this->transmit(message, needsACK);
	}

	//----------
	void
		RS485::transmit(const MsgpackBinary& packetContent, bool needsACK)
	{
		if (!this->isConnected()) {
			ofLogError() << "Cannot transmit when serial is closed";
			return;
		}

		this->serialThread->outbox.send({ packetContent, needsACK });
	}

	//----------
	void
		RS485::transmitPing(const Target& target)
	{
		this->transmit(msgpack11::MsgPack::array{
			(int8_t)target
			, (int8_t)0
			, msgpack11::MsgPack()
			});
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
		if (this->parameters.debug.printResponses.get()) {
			cout << "Rx : " << json.dump(4) << endl;
		}

		this->app->processIncoming(json);
	}

	//----------
	void
		RS485::openSerial(int deviceID)
	{
		if (this->serialThread) {
			this->closeSerial();
		}
		auto serialThread = make_shared<SerialThread>();

		serialThread->serial.setup(deviceID, this->parameters.baudRate.get());
		if (!serialThread->serial.isInitialized()) {
			// could also throw an exception at this point
			return;
		}

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
		this->serialThread->serial.close();
		this->serialThread.reset();
	}

	//----------
	void
		RS485::serialThreadedFunction()
	{
		while (!this->serialThread->joining) {
			auto didRx = this->serialThreadReceive();
			auto didTx = this->serialThreadSend();

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
		if (!this->serialThread->serial.available()) {
			return false;
		}

		while (this->serialThread->serial.available()) {
			auto word = this->serialThread->serial.readByte();

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

			// Send the data to serial
			auto bytesWritten = this->serialThread->serial.writeBytes(binaryCOBS.data()
				, binaryCOBS.size());

			// Check that all bytes were sent
			if (bytesWritten != binaryCOBS.size()) {
				ofLogError("RS485") << "Failed to write all " << binaryCOBS.size() << " bytes to stream";
			}

			// Send the EOP (this doesn't come with cobs-c
			this->serialThread->serial.writeByte((unsigned char)0);

			// Store the messagepack representation in case the user wants to debug it
			this->debug.lastMessagePackTx.assign(data, data + size);

			this->debug.isFrameNewMessageTx.notify();
			this->debug.txCount++;

			// Wait for 10ms after each send
			this_thread::sleep_for(chrono::milliseconds(this->parameters.gapBetweenSends_ms.get()));

			// After we send, we try to receive for up to the duration of the response window
			if(packet.needsACK) {
				auto responseWindowDuration = chrono::milliseconds(this->parameters.responseWindow_ms.get());
				auto responseWindowEnd = chrono::system_clock::now() + responseWindowDuration;

				bool timeOut = true;
				while (chrono::system_clock::now() < responseWindowEnd) {
					if (this->serialThreadReceive()) {
						// break on first response
						timeOut = false;
						break;
					}
					this_thread::sleep_for(chrono::milliseconds(1));
				}

				if (timeOut) {
					ofLogError("RS485") << "Timeout waiting for response";
				}
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

				{
					const bool debugBrokenMessagePack = this->parameters.debug.printResponses.get();
					if (debugBrokenMessagePack) {
						cout << "msgpack : ";
						for (const auto& byte : msgpackBinary) {
							printChar(byte);
						}
						cout << endl;
					}
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