#include "pch_App.h"
#include "RS485.h"
#include "Column.h"
#include "../SerialDevices/listDevices.h"
#include "../SerialDevices/Factory.h"

#include "../cobs-c/cobs.h"
#include "msgpack.h"

// We include crow for base64 functions
#include "crow/crow/utility.h"

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

	//----------
	RS485::Packet::Packet(const function<msgpack11::MsgPack()>& lazyMessageRenderer)
		: lazyMessageRenderer(lazyMessageRenderer)
	{

	}

	//----------
	void
		RS485::Packet::render()
	{
		if (this->lazyMessageRenderer) {
			auto message = this->lazyMessageRenderer();
			auto dataString = message.dump();
			auto dataBegin = (uint8_t*)dataString.data();
			auto dataEnd = dataBegin + dataString.size();
			this->msgpackBinary.assign(dataBegin, dataEnd);
		}
	}

#pragma mark RS485
	//----------
	RS485::RS485(Column* column)
		: column(column)
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
		RS485::deserialise(const nlohmann::json& json)
	{
		this->openSerial(json);
		this->initilialisation.settings = json;
		this->initilialisation.lastConnectionAttempt = chrono::system_clock::now();
	}

	//----------
	void
		RS485::update()
	{
		// Check connection
		{
			// Close serial if disconnected
			if (this->serialThread) {
				if (!this->serialThread->serialDevice->isConnected()) {
					this->closeSerial();
				}
			}

			// Check if can reconnect
			if (!this->serialThread
				&& !this->initilialisation.settings.empty()
				&& chrono::system_clock::now() - this->initilialisation.lastConnectionAttempt > this->initilialisation.retryPeriod) {
				this->openSerial(this->initilialisation.settings);
				this->initilialisation.lastConnectionAttempt = chrono::system_clock::now();
			}
		}

		// Pull and process the inbox
		this->updateInbox();

		// Collate packets in the outbox
		if (this->parameters.collatePackets) {
			this->collateOutboxPackets();
		}

		// Update indicators
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
			{
				auto stack = inspector->addHorizontalStack();
				{
					auto widget = make_shared<ofxCvGui::Widgets::LiveValue<string>>("Serial device type", [this]() {
						if (this->serialThread) {
							return this->serialThread->serialDevice->getTypeName();
						}
						else {
							return string("None");
						}
						});
					stack->add(widget);
				}
				{
					auto widget = make_shared<ofxCvGui::Widgets::LiveValue<string>>("Serial device address", [this]() {
						if (this->serialThread) {
							return this->serialThread->serialDevice->getAddressString();
						}
						else {
							return string("");
						}
						});
					stack->add(widget);
				}
			}
			inspector->addButton("Disconnect", [this]() {
				this->closeSerial();
				this->initilialisation.settings = nlohmann::json(); // clear settings so don't auto-init
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
			auto widgets = this->getWidgets();
			for (auto widget : widgets) {
				inspector->add(widget);
			}
		}

		inspector->addButton("Clear outbox", [this]() {
			this->clearOutbox();
			});

		inspector->addButton("Clear counters", [this]() {
			this->clearCounters();
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
	size_t RS485::getOutboxCount() const
	{
		if (this->serialThread) {
			return this->serialThread->outbox.size();
		}
		else {
			return (size_t)0;
		}
	}

	//----------
	void
		RS485::clearOutbox()
	{
		if (!this->isConnected()) {
			return;
		}

		// just receive all the packets here
		Packet _;
		while (this->serialThread->outbox.tryReceive(_)) {
			// do nothing with these messages, just clear them out
		}
	}

	//----------
	void
		RS485::clearCounters()
	{
		this->debug.rxCount = 0;
		this->debug.txCount = 0;
	}

	//----------
	void
		RS485::processIncoming(const nlohmann::json& json)
	{
		if (this->parameters.debug.printRx.get()) {
			cout << "Rx : " << json.dump(4) << endl;
		}

		this->column->processIncoming(json);
	}

	//----------
	vector<ofxCvGui::ElementPtr>
		RS485::getWidgets()
	{
		return {
			make_shared<ofxCvGui::Widgets::Heartbeat>("Rx", [this]() {
				return this->debug.isFrameNewMessageRx.isFrameNew;
				})
			, make_shared<ofxCvGui::Widgets::LiveValue<size_t>>("Rx count", [this]() {
				return this->debug.rxCount;
			})
			, make_shared<ofxCvGui::Widgets::Heartbeat>("Tx", [this]() {
				return this->debug.isFrameNewMessageTx.isFrameNew;
			})
			, make_shared<ofxCvGui::Widgets::LiveValue<size_t>>("Tx count", [this]() {
				return this->debug.txCount;
			})
			, make_shared<ofxCvGui::Widgets::LiveValue<size_t>>("Outbox count", [this]() {
				return this->getOutboxCount();
			})
		};
	}


	//----------
	void
		RS485::collateOutboxPackets()
	{
		if (!this->serialThread) {
			return;
		}

		vector<RS485::Packet> allPackets;
		{
			Packet packet;
			while (this->serialThread->outbox.tryReceive(packet)) {
				allPackets.push_back(packet);
			}
		}
		
		vector<RS485::Packet> filteredPackets;

		for (const auto& packet : allPackets) {
			bool foundMatch = false;
			for (auto& existingPacket : filteredPackets) {
				if (packet.collateable
					&& existingPacket.collateable
					&& existingPacket.address == packet.address
					&& existingPacket.target == packet.target)
				{
					// overwrite the packet
					existingPacket = packet;
					foundMatch = true;
					break;
				}
			}
			if (!foundMatch) {
				// add the packet
				filteredPackets.push_back(packet);
			}
		}

		for (const auto& packet : filteredPackets) {
			this->serialThread->outbox.send(packet);
		}
	}

	//----------
	bool
		RS485::hasRxBeenReceived() const
	{
		return this->debug.hasRxBeenReceived;
	}

	//----------
	void
		RS485::openSerial(const SerialDevices::ListedDevice& listedDevice)
	{
		auto serialDevice = listedDevice.createDevice();
		if (!serialDevice) {
			ofLogError() << "Could not open serial device " + listedDevice.type + "::" + listedDevice.name;
			return;
		}

		this->openSerial(serialDevice);
	}

	//----------
	void
		RS485::openSerial(const nlohmann::json& json)
	{
		auto serialDevice = SerialDevices::createFromJson(json);
		if (!serialDevice) {
			ofLogError() << "Could not open serial device from json : " + json.dump(4);
			return;
		}

		this->openSerial(serialDevice);
	}

	//----------
	void
		RS485::openSerial(shared_ptr<SerialDevices::IDevice> serialDevice)
	{
		if (this->serialThread) {
			this->closeSerial();
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
			// Perform any actions that should happen in serialThread
			{
				std::function<void()> action;
				while (this->serialThreadActions.tryReceive(action)) {
					action();
				}
			}

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
				if (decodeResult.status != COBS_DECODE_OK
					&& this->parameters.debug.printMessageErrors) {
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

				// Decode messagepack
				nlohmann::json json;
				try {
					json = nlohmann::json::from_msgpack(binaryMessagePack);

					if (json.size() >= 3) {
						// note who has replied
						this->repliesSeenFrom.push_back((int) json[1]);
					}
				}
				catch (const std::exception& e) {
					ofLogError("LaserSystem") << "msgpack deserialize error : " << e.what();

					if (this->parameters.debug.printBrokenMsgpack.get()) {
						cout << "msgpack : ";
						for (const auto& byte : binaryMessagePack) {
							printChar(byte);
						}
						cout << endl;
					}

					this->debug.isFrameNewMessageRxError.notify();

					continue;
				}

				this->serialThread->inbox.send(json);
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

		// Send packets
		Packet packet;
		while (this->serialThread->outbox.tryReceive(packet)) {
			if (this->serialThread->joining) {
				// if we're exiting, just ignore rest of packets in outbox
				break;
			}

			// For lazy packets
			packet.render();

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
				this->repliesSeenFrom.clear();
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

						if (this->parameters.debug.printBase64.get()) {
							cout << crow::utility::base64encode(binaryCOBS.data(), binaryCOBS.size()) << endl;
						}
					}

					{
						cout << "Tx msgpack : ";
						for (const auto& byte : msgpackBinary) {
							printChar(byte);
						}
						cout << endl;

						if (this->parameters.debug.printBase64.get()) {
							cout << crow::utility::base64encode(msgpackBinary.data(), msgpackBinary.size()) << endl;
						}
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
				auto startTime = chrono::system_clock::now();
				auto responseWindowEnd = startTime + duration;
				while (chrono::system_clock::now() < responseWindowEnd) {
					this->serialThreadReceive();

					// see if we got a message
					{
						for(const auto receivedMessageFrom : this->repliesSeenFrom) {
							if (receivedMessageFrom == senderID) {
								if (this->parameters.debug.printACKTime.get()) {
									auto duration = chrono::system_clock::now() - startTime;
									auto ms = chrono::duration_cast<chrono::milliseconds>(duration).count();
									cout << "ACK received in " << ms << "ms" << endl;
								}
								return true;
							}
						}

						this_thread::sleep_for(chrono::microseconds(100));
					}
				}

				return false;
			};

			// After we send, we try to receive for up to the duration of the response window
			if (packet.needsACK) {
				auto waitDuration = packet.customWaitTime_ms > 0
					? chrono::milliseconds(packet.customWaitTime_ms)
					: chrono::milliseconds(this->parameters.responseWindow_ms.get());

				if (!waitForReceive(packet.target, waitDuration)
					&& this->parameters.debug.printMessageErrors) {
					ofLogError() << "ACK not seen from " << packet.target;
				}
			}
			else if (packet.customWaitTime_ms == 0)
			{
				// don't wait
			}
			else {
				// We just always wait in this case
				auto waitDuration = packet.customWaitTime_ms >= 0
					? chrono::milliseconds(packet.customWaitTime_ms)
					: chrono::milliseconds(this->parameters.gapBetweenBroadcastSends_ms.get());

				this_thread::sleep_for(waitDuration);
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

		nlohmann::json json;

		while (this->serialThread->inbox.tryReceive(json)) {
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
			this->debug.hasRxBeenReceived = true;
		}
	}
}