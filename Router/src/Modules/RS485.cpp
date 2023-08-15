#include "pch_App.h"
#include "RS485.h"
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
	RS485::RS485()
	{

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
		this->receive();

		{
			this->debug.isFrameNewMessageRx.update();
			this->debug.isFrameNewMessageTx.update();
			this->debug.isFrameNewMessageRxError.update();
			this->debug.isFrameNewDeviceRxSuccess.update();
			this->debug.isFrameNewDeviceRxFail.update();
			this->debug.isFrameNewDeviceError.update();
		}

		if (this->parameters.debug.flood) {
			if (this->serial.isInitialized()) {
				this->serial.writeByte((char) 100);
			}
		}
	}

	//----------
	void
		RS485::populateInspector(ofxCvGui::InspectArguments& args)
	{
		auto inspector = args.inspector;

		inspector->addTitle("Device");
		inspector->addIndicatorBool("Connected", [this]() {
			return this->serial.isInitialized();
			});
		if (!this->serial.isInitialized()) {

			inspector->addTitle("Connect to", ofxCvGui::Widgets::Title::Level::H2);

			auto devices = this->serial.getDeviceList();
			for (auto device : devices) {
				auto deviceName = device.getDeviceName();
				auto deviceID = device.getDeviceID();
				inspector->addButton(deviceName, [this, deviceID]() {
					this->serial.setup(deviceID, 115200);
					ofxCvGui::refreshInspector(this);
					});
			}
		}
		else {
			inspector->addLiveValue<string>("Device name", [this]() {
				return this->connectedPortName;
				});
			inspector->addButton("Disconnect", [this]() {
				this->serial.close();
				ofxCvGui::refreshInspector(this);
				});
		}

		inspector->addSpacer();

		if (this->serial.isInitialized()) {
			inspector->addButton("Send poll", [this]() {
				this->transmitPoll((Target) this->parameters.debug.targetID.get());
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
	vector<uint8_t>
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
		vector<uint8_t> header;
		header.assign(headerBuffer.data, headerBuffer.data + headerBuffer.size);
		msgpack_sbuffer_destroy(&headerBuffer);

		return header;
	}

	//----------
	void
		RS485::transmitRawPacket(const uint8_t* data, size_t size)
	{
		// allocate buffer of max size for cobs
		vector<uint8_t> binaryCOBS(size * 255 / 254 + 2);

		// Perform the encode
		auto encodeResult = cobs_encode(binaryCOBS.data()
			, binaryCOBS.size()
			, data
			, size);

		// Check we encoded OK
		if (encodeResult.status != COBS_ENCODE_OK) {
			ofLogError("LaserSystem") << "Failed to encode COBS : " << encodeResult.status;
			return;
		}

		// Crop the message to the correct number of bytes
		binaryCOBS.resize(encodeResult.out_len);

		// Send the data to serial
		auto bytesWritten = this->serial.writeBytes(binaryCOBS.data(), binaryCOBS.size());

		// Check that all bytes were sent
		if (bytesWritten != binaryCOBS.size()) {
			ofLogError("LaserSystem") << "Failed to write all " << binaryCOBS.size() << " bytes to stream";
		}

		// Send the EOP (this doesn't come with cobs-c
		this->serial.writeByte((unsigned char)0);

		// Store the messagepack representation in case the user wants to debug it
		this->debug.lastMessagePackTx.assign(data, data + size);

		this->debug.isFrameNewMessageTx.notify();
		this->debug.txCount++;
	}

	//----------
	void
		RS485::transmitPoll(const Target& target)
	{
		// Create header
		auto header = RS485::makeHeader(target);

		// Create body
		vector<uint8_t> body;
		{
			msgpack_sbuffer buffer;
			msgpack_packer packer;
			msgpack_sbuffer_init(&buffer);
			msgpack_packer_init(&packer
				, &buffer
				, msgpack_sbuffer_write);

			msgpack_pack_nil(&packer);

			body.assign((uint8_t*) buffer.data
				, (uint8_t*) (buffer.data + buffer.size));

			msgpack_sbuffer_destroy(&buffer);
		}

		// Transmit
		this->transmitHeaderAndBody(header, body);
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
		RS485::transmitHeaderAndBody(const vector<uint8_t>& header
			, const vector<uint8_t>& body)
	{
		// Combine header and body
		vector<uint8_t> headerAndBody = header;
		headerAndBody.insert(headerAndBody.end()
			, body.begin()
			, body.end());

		this->transmitRawPacket(headerAndBody.data()
			, headerAndBody.size());
	}

	//----------
	void
		RS485::receive()
	{
		if (!this->serial.isInitialized()) {
			return;
		}

		while (this->serial.available()) {
			auto word = this->serial.readByte();

			// If end of COB packet
			if (word == 0) {
				// If there's nothing to decode, don't do anything
				if (this->cobsIncoming.empty()) {
					continue;
				}

				// Clear incoming buffer (and keep a copy for use here)
				auto copyOfIncoming = this->cobsIncoming;
				this->cobsIncoming.clear();

				if (this->isFirstIncoming)
				{
					// Ignore first message (usually it's partial message)
					this->isFirstIncoming = false;
				}
				else {
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

					// Decode messagepack
					nlohmann::json json;
					try {
						json = nlohmann::json::from_msgpack(binaryMessagePack);
					}
					catch (const std::exception& e) {
						ofLogError("LaserSystem") << "msgpack deserialize error : " << e.what();

						{
							const bool debugBrokenMessagePack = true;
							if (debugBrokenMessagePack) {
								cout << "COBS : ";
								for (const auto& byte : copyOfIncoming) {
									printChar(byte);
								}
								cout << endl;

								cout << "msgpack : ";
								for (const auto& byte : binaryMessagePack) {
									printChar(byte);
								}
								cout << endl;
							}
						}

						this->debug.isFrameNewMessageRxError.notify();

						continue;
					}

					// Perform deserialize on json
					this->processIncoming(json);
					this->lastIncomingMessageTime = std::chrono::system_clock::now();
					this->debug.isFrameNewMessageRx.notify();
					this->debug.rxCount++;
				}
			}
			// Continuation of COB packet
			else {
				this->cobsIncoming.push_back(word);
			}
		}
	}

	//----------
	void
		RS485::processIncoming(const nlohmann::json& json)
	{
		cout << "Rx : " << json.dump(4) << endl;
	}
}