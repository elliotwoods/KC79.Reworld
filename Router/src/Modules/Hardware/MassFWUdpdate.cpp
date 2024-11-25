#include "pch_App.h"
#include "MassFWUpdate.h"
#include "../App.h"
#include "../../Utils.h"

namespace Modules {
	namespace Hardware {
		//----------
		MassFWUpdate::MassFWUpdate()
		{

		}

		//----------
		string
			MassFWUpdate::getTypeName() const
		{
			return "MassFWUpdate";
		}

		//----------
		void
			MassFWUpdate::init()
		{
			this->onPopulateInspector += [this](ofxCvGui::InspectArguments& args) {
				this->populateInspector(args);
				};
		}

		//----------
		void
			MassFWUpdate::update()
		{
			if (this->parameters.announce.enabled) {
				auto nextSend = this->announce.lastSend + chrono::milliseconds((int)(this->parameters.announce.period.get() * 1000.0f));
				if (nextSend <= chrono::system_clock::now()) {
					auto rs485s = this->getRS485s();
					for (auto rs485 : rs485s) {
						this->announceFirmware(rs485);
					}
				}
			}

		}

		//----------
		void
			MassFWUpdate::populateInspector(ofxCvGui::InspectArguments& args)
		{
			auto inspector = args.inspector;
			inspector->addParameterGroup(this->parameters);

			{
				auto button = inspector->addButton("Upload files", [this]() {
					auto result = ofSystemLoadDialog("Select application.bin file");
					if (result.bSuccess) {
						this->uploadFirmware(result.filePath);
					}
					}, OF_KEY_RETURN);
				button->setHeight(150.0f);
			}

			inspector->addButton("Erase flash", [this]() {
				auto rs485s = this->getRS485s();
				for (auto rs485 : rs485s) {
					this->eraseFirmware(rs485);
				}
				});
			inspector->addButton("Run application", [this]() {
				auto rs485s = this->getRS485s();
				for (auto rs485 : rs485s) {
					this->runApplication(rs485);
				}
				}, 'r');

		}

		//----------
		// sequence : https://paper.dropbox.com/doc/KC79-Firmware-development-log--B~RnAsj4dL_fF81TgG7UhvbbAg-NaTWt2IkZT4ykJZeMERKP#:uid=689242674973595348642171&h2=Sequence
		void
			MassFWUpdate::uploadFirmware(const string& path, const function<void(const string&)>& onProgress)
		{
			// Make an onProgress if there isn't one
			auto progressAction = onProgress
				? onProgress
				: [](string notice) {
				ofxCvGui::Utils::drawProcessingNotice(notice);
				};

			auto moduleName = "MassFWUpdate::uploadFirmware";

			// Gather RS485 connections
			auto rs485s = this->getRS485s();

			vector<uint8_t> data;
			{
				// Read contents from file
				{
					auto file = ofFile(path);
					auto fileBuffer = file.readToBuffer();
					file.close();
					auto size = fileBuffer.size();

					if (fileBuffer.size() == 0) {
						ofLogError("MassFWUpdate") << "Couldn't read file contents";
						return;
					}

					auto rawData = (uint8_t*)fileBuffer.getData();
					data.assign(rawData, rawData + fileBuffer.size());
				}

				// Truncate all 0xFFs from end of firmware
				if (this->parameters.upload.truncate.get()) {
					auto lastFF = data.size();
					while (data[lastFF - 1] == 0xFF) {
						lastFF--;
					}

					data.resize(lastFF);
				}
			}

			// 0. Clear any existing messages in outboxs
			{
				for (auto rs485 : rs485s) {
					rs485->clearOutbox();
				}
			}

			// 1. First announce it
			{
				progressAction("Announcing firmware");
				for (int i = 0; i < 50; i++) {
					for (auto rs485 : rs485s) {
						this->announceFirmware(rs485);
					}
					ofSleepMillis(100);
				}
			}

			// 2. Erase the existing firmware (required before uploading)
			{
				progressAction("Erasing flash");
				for (auto rs485 : rs485s) {
					for (int i = 0; i < this->parameters.upload.frameRepetitions.get(); i++) {
						this->eraseFirmware(rs485);
					}
				}

				// Send announcements whilst we're waiting for erase to finish
				for (int i = 0; i < 50; i++) {
					for (auto rs485 : rs485s) {
						this->announceFirmware(rs485);
					}
					ofSleepMillis(100);
				}
			}

			// 3. Upload
			{
				progressAction("Uploading");

				auto dataPosition = data.data();
				auto dataEnd = dataPosition + data.size();
				uint32_t packetIndex = 0;
				const auto frameSize = this->parameters.upload.frameSize;
				uint32_t frameOffset = 0;

				while (dataPosition < dataEnd) {
					auto remainingSize = dataEnd - dataPosition;
					{
						progressAction("Uploading : " + ofToString(remainingSize / 1024, 1) + "kB remaining");
					}


					if (remainingSize > frameSize) {
						// Normal packets

						for (int i = 0; i < this->parameters.upload.frameRepetitions.get(); i++) {
							for (auto rs485 : rs485s) {
								uploadFirmwarePacket(rs485
									, frameOffset
									, dataPosition
									, FW_FRAME_SIZE);
							}
						}


						dataPosition += frameSize;
						frameOffset += frameSize;
					}
					else {
						// Last packet

						for (int i = 0; i < this->parameters.upload.frameRepetitions.get(); i++) {
							for (auto rs485 : rs485s) {
								uploadFirmwarePacket(rs485
									, frameOffset
									, dataPosition
									, remainingSize);
							}
						}
						dataPosition += remainingSize;
						frameOffset += remainingSize;
					}

				}
			}

			ofSleepMillis(1000);

			// 4. Disable announce (eventually app will run)
			{
				this->parameters.announce.enabled = false;
			}
		}

		//----------
		vector<shared_ptr<RS485>>
			MassFWUpdate::getRS485s() const
		{
			auto moduleName = "MassFWUpdate::getRS485s";

			vector<shared_ptr<RS485>> rs485s;
			{
				auto columns = App::X()->getInstallation()->getAllColumns();

				for (auto column : columns) {
					auto rs485 = column->getRS485();
					if (!rs485->isConnected()) {
						ofLogWarning(moduleName) << "Column " << column->getName() << " RS485 not connected";
						continue;
					}
					rs485s.push_back(rs485);
				}
			}
			return rs485s;
		}

		//----------
		void
			MassFWUpdate::announceFirmware(shared_ptr<RS485> rs485)
		{
			this->sendMagicWord(rs485, 'F', 'W');
		}

		//----------
		void
			MassFWUpdate::eraseFirmware(shared_ptr<RS485> rs485)
		{
			this->sendMagicWord(rs485, 'E', 'R');
		}

		//----------
		void
			MassFWUpdate::runApplication(shared_ptr<RS485> rs485)
		{
			this->sendMagicWord(rs485, 'R', 'U');
		}

		//----------
		void
			MassFWUpdate::sendMagicWord(shared_ptr<RS485> rs485, char a, char b)
		{
			if (!rs485) {
				ofLogError("No RS485");
				return;
			}
			if (!rs485->isConnected()) {
				// No connection
				return;
			}

			msgpack_sbuffer messageBuffer;
			msgpack_packer packer;
			msgpack_sbuffer_init(&messageBuffer);
			msgpack_packer_init(&packer
				, &messageBuffer
				, msgpack_sbuffer_write);

			msgpack_pack_array(&packer, 3);
			{
				// First element is target address
				msgpack_pack_fix_int8(&packer, -1);

				// Second element is source address
				msgpack_pack_fix_int8(&packer, 0);

				// Third element is the message body
				string magicWord;
				magicWord.resize(2);
				magicWord[0] = a;
				magicWord[1] = b;
				msgpack_pack_str(&packer, magicWord.size());
				msgpack_pack_str_body(&packer, magicWord.c_str(), magicWord.size());
			}

			// Send packet
			{
				auto packet = RS485::Packet(messageBuffer);
				packet.needsACK = false;
				packet.customWaitTime_ms = this->parameters.upload.waitBetweenFrames.get();

				// send and wait for complete
				{
					std::promise<void> promise;
					packet.onSent = [&promise]() {
						promise.set_value();
						};
					rs485->transmit(packet);

					// wait for packet to be sent
					promise.get_future().get();
				}
			}

			msgpack_sbuffer_destroy(&messageBuffer);

			this->announce.lastSend = chrono::system_clock::now();
		}

		//----------
		void
			MassFWUpdate::uploadFirmwarePacket(shared_ptr<RS485> rs485
				, uint32_t frameOffset
				, uint8_t* packetData
				, size_t packetSize)
		{
			// Prepend the data with checksum
			auto checksum = Utils::calcCheckSum((uint8_t*)packetData, packetSize);
			auto checksumBytes = (uint8_t*)&checksum;
			vector<uint8_t> packetBody;
			for (int i = 0; i < sizeof(Utils::CRCType); i++) {
				packetBody.push_back(checksumBytes[i]);
			}


			// Add the data to the packet
			packetBody.insert(packetBody.end()
				, packetData
				, packetData + packetSize);

			// Transmit via msgpack
			{
				msgpack_sbuffer messageBuffer;
				msgpack_packer packer;
				msgpack_sbuffer_init(&messageBuffer);
				msgpack_packer_init(&packer
					, &messageBuffer
					, msgpack_sbuffer_write);

				// The broadcast packet header
				msgpack_pack_array(&packer, 3);
				{
					// First element is target address
					msgpack_pack_fix_int8(&packer, -1);

					// Second element is source address
					msgpack_pack_fix_int8(&packer, 0);

					// Third element is the message body
					msgpack_pack_map(&packer, 1);
					{
						// Key is the packet index
						if (sizeof(Utils::CRCType) == 2) {
							msgpack_pack_uint32(&packer, frameOffset);
						}
						else if (sizeof(Utils::CRCType) == 4) {
							msgpack_pack_uint32(&packer, frameOffset);
						}

						// Value is the data
						msgpack_pack_bin(&packer, packetBody.size());
						msgpack_pack_bin_body(&packer, packetBody.data(), packetBody.size());
					}

				}

				// Send packet
				{
					auto packet = RS485::Packet(messageBuffer);
					packet.needsACK = false;
					packet.customWaitTime_ms = this->parameters.upload.waitBetweenFrames.get();
					packet.collateable = false;

					// send and wait for complete
					std::promise<void> promise;
					packet.onSent = [&promise]() {
						promise.set_value();
						};
					rs485->transmit(packet);

					// wait for packet to be sent
					promise.get_future().get();
				}

				msgpack_sbuffer_destroy(&messageBuffer);
			}
		}
	}
}
