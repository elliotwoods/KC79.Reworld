#include "RS485.h"
#include <Arduino.h>
#include <string.h>

#include <msgpack.hpp>

#include "App.h"
#include "Logger.h"
#include "Exception.h"
#include "../Handoff.h"

HardwareSerial serialRS485(PA3, PA2);
msgpack::COBSRWStream cobsStream(serialRS485);

#define PIN_DE PA1
namespace Modules {
	//---------
	RS485 * RS485::instance = nullptr;

	//---------
	RS485::RS485(App * app)
	: app(app)
	{
		RS485::instance = this;
	}

	//----------
	const char *
	RS485::getTypeName() const
	{
		return "RS485";
	}

	//---------
	void
	RS485::setup()
	{
		serialRS485.begin(115200);

		// Setup the DE pin
		pinMode(PIN_DE, OUTPUT);
		digitalWrite(PIN_DE, LOW);
	}

	//---------
	void
	RS485::update()
	{
		this->processIncoming();
	}

	//---------
	void
	RS485::sendStatusReport()
	{
		this->beginTransmission();

		const auto ourID = this->app->id->get();

		// Packer [target, sender, message, seq, crc16]
		msgpack::writeArraySize4(cobsStream, 5);
		{
			msgpack::writeInt8(cobsStream, 0);
			msgpack::writeInt8(cobsStream, ourID);

			// From here we use Serializer
			msgpack::Serializer serializer(cobsStream);
			this->app->reportStatus(serializer);
		}

		this->finishFrame();
	}

	//---------
	void
	RS485::sendPositions()
	{
		// If we're doing this in response to a message, then no other ACK is required
		RS485::noACKRequired();

		this->beginTransmission();

		const auto ourID = this->app->id->get();

		// Packer [target, sender, message, seq, crc16]
		msgpack::writeArraySize4(cobsStream, 5);
		{
			msgpack::writeInt8(cobsStream, 0);
			msgpack::writeInt8(cobsStream, ourID);

			msgpack::writeMapSize4(cobsStream, 1);
			{
				// Key
				msgpack::writeString5(cobsStream, "p", 1);

				// Value
				msgpack::writeArraySize4(cobsStream, 4);
				{
					msgpack::writeInt32(cobsStream, this->app->motionControlA->getPosition());
					msgpack::writeInt32(cobsStream, this->app->motionControlB->getPosition());
					msgpack::writeInt32(cobsStream, this->app->motionControlA->getTargetPosition());
					msgpack::writeInt32(cobsStream, this->app->motionControlB->getTargetPosition());
				}
			}
		}

		this->finishFrame();
	}

	//---------
	void
	RS485::sendBootloaderImageStatus(uint8_t state, uint32_t length, uint32_t received)
	{
		// A reply, not an ACK, so this frame carries information the bare bool cannot.
		RS485::noACKRequired();

		this->beginTransmission();

		const auto ourID = this->app->id->get();

		msgpack::writeArraySize4(cobsStream, 5);
		{
			msgpack::writeInt8(cobsStream, 0);
			msgpack::writeInt8(cobsStream, ourID);

			msgpack::writeMapSize4(cobsStream, 1);
			{
				msgpack::writeString5(cobsStream, "blimg", 5);
				msgpack::writeMapSize4(cobsStream, 3);
				{
					msgpack::writeString5(cobsStream, "st", 2);
					msgpack::writeIntU8(cobsStream, state);
					msgpack::writeString5(cobsStream, "len", 3);
					msgpack::writeIntU32(cobsStream, length);
					msgpack::writeString5(cobsStream, "n", 1);
					msgpack::writeIntU32(cobsStream, received);
				}
			}
		}

		this->finishFrame();
	}

	//---------
	void
	RS485::sendACKEarly(bool success)
	{
		RS485::instance->sendACK(success);
		RS485::instance->sentACKEarly = true;
	}

	//---------
	void
	RS485::noACKRequired()
	{
		RS485::instance->disableACK = true;
	}

	//---------
	bool
	RS485::replyAllowed()
	{
		return !RS485::instance->disableACK && !RS485::instance->sentACKEarly;
	}

	//---------
	bool
	RS485::checkChecksum()
	{
		if(!RS485::instance->verifyChecksumEnabled) {
			return true;
		}
		uint8_t seq;
		if(!cobsStream.checkChecksum(seq)) {
			return false;
		}
		RS485::instance->lastRxSeq = seq;
		return true;
	}

	//---------
	void
	RS485::setVerifyChecksumEnabled(bool value)
	{
		RS485::instance->verifyChecksumEnabled = value;
	}

	//---------
	bool
	RS485::getVerifyChecksumEnabled()
	{
		return RS485::instance->verifyChecksumEnabled;
	}

	//---------
	bool
	RS485::hasAnySignalBeenReceived() const
	{
		return this->anySignalReceived;
	}

	//---------
	void
	RS485::processIncoming()
	{
		const auto ourID = this->app->id->get();

		// Skip any partial packets
		if(!cobsStream.isStartOfIncomingPacket()) {
			cobsStream.nextIncomingPacket();
		}

		while(cobsStream.isStartOfIncomingPacket() && cobsStream.available()) {
			bool needsReply = false;

			// set the flags for ACKS (used inside processIncoming under processCOBSPacket)
			this->sentACKEarly = false;
			this->disableACK = false;

			// this will be raised inside processCOBSPacket if packet is for us exclusively
			bool isForUs = false;

			auto exception = this->processCOBSPacket(isForUs);
			if(exception) {
				log(exception);
			}
			else {
				this->anySignalReceived = true;
			}

			// disable ACK is handled inside sendACK function
			if(isForUs) {
				if(exception) {
					this->sendACK(false);
				}
				else {
					this->sendACK(true);
				}
			}

			cobsStream.nextIncomingPacket();
		}
	}

	//---------
	Exception
	RS485::processCOBSPacket(bool & isForUs)
	{
		// create a moduleName
		char moduleName[100];
		sprintf(moduleName, "%s.processCOBSPacket", this->getName());

		// Check it's a message for us
		bool weShouldProcess = false;
		{
			// We're expecting a 3-element array
			{
				// Note that these exceptions should not be thrown as
				// error ACKs, as they will conflict with ACKs coming
				// from other modules at the same time.
				size_t arraySize;
				if(!msgpack::readArraySize(cobsStream, arraySize)) {
					return Exception::MessageFormatError(moduleName);
				};
				if(arraySize < 3) {
					return Exception::MessageFormatError(moduleName);
				}
			}

			// First element is target address
			int8_t targetAddress;
			{
				if(!msgpack::readInt<int8_t>(cobsStream, targetAddress)) {
					return Exception::MessageFormatError(moduleName);
				}

				if(targetAddress == this->app->id->get()) {
					isForUs = true;
				}

				// An address of -1 means it's addressed to all devices
				// We process but we don't ACK
				if(targetAddress == -1) {
					isForUs = true;
					this->disableACK = true;
				}
			}

			// Second element is the source address (we ignore)
			{
				int8_t _;
				if(!msgpack::readInt<int8_t>(cobsStream, _)) {
					return Exception::MessageFormatError(moduleName);
				}
			}

			if(isForUs) {
				// We should process this packet
				// There are different types of packet:

				if(msgpack::nextDataTypeIs(cobsStream, msgpack::DataType::Nil)) {
					// If it's a Nil, then it's a ping
					if(!msgpack::readNil(cobsStream)) {
						return Exception::MessageFormatError(moduleName);
					}
					// Will result in an ACK being sent (the ping reply)
				}
				else if(msgpack::nextDataTypeIs(cobsStream, msgpack::DataType::String5)) {
					// If it's a string5, it's a magic word
					char word[64];
					uint8_t wordSize;
					if(!msgpack::readString5(cobsStream, word, 64, wordSize)) {
						return Exception::MessageFormatError(moduleName);
					}
					// A longer, improbable token rather than the old bare "FW" (2 bytes) --
					// the bootloader's own announce word stays "FW" (it's frozen, field-burned,
					// and only re-flashable via ST-Link), so this only guards the running
					// application against an accidental/corrupt 2-byte match bouncing it into
					// the bootloader mid-move. The Router announces this word first to bump
					// every running app into its bootloader, then continues with the legacy
					// "FW" for the bootloader itself.
					if(wordSize == 7 && memcmp(word, "FW!KC79", 7) == 0) {
						// The magic word is the entire body (a bare string) -- nothing else
						// follows it, so the stream is exactly at the trailer here, and this
						// is the single highest-priority place for the commit gate: an
						// unauthenticated reboot-to-bootloader is exactly what Finding 4 in
						// protocol-hardening.md is about. No-ops (returns true) until
						// verifyChecksumEnabled is turned on -- see RS485::checkChecksum().
						if(!RS485::checkChecksum()) {
							return Exception(moduleName, "Checksum FAIL");
						}
						// Firmware announce packet. Reset into the bootloader, leaving it a
						// note: the bootloader has no way of its own to learn this board's RS485
						// address -- the daisy-chain that assigns it is run by this image -- so
						// without the handoff it can only be addressed as part of an anonymous
						// broadcast crowd, which is what made the old update protocol unsteerable.
						// STAY also buys it a thirty-second window instead of three.
						Handoff::write(this->app->id->get(),
							this->app->getProvisionSerial(),
							PORTAL_HANDOFF_REQUEST_STAY);
						log(LogLevel::Status, moduleName, "Firmware announced, rebooting...");
						HAL_Delay(500);
						NVIC_SystemReset();
					}
				}
				else {
					// If it's a map, it's a message for the app
					auto success = app->processIncoming(cobsStream);
					if(!success) {
						return Exception::MessageFormatError(moduleName);
					}
				}
			}
		}

		return Exception::None();
	}

	//---------
	void
	RS485::beginTransmission()
	{
		
	 	digitalWrite(PIN_DE, HIGH);
	}

	//---------
	void
	RS485::endTransmission()
	{
		cobsStream.flush();
		digitalWrite(PIN_DE, LOW);
	}

	//---------
	void
	RS485::finishFrame()
	{
		// [..., seq, crc16] -- seq echoes the last request this device successfully verified
		// (see checkChecksum()); crc16 is a snapshot of the running CRC taken right after
		// writing seq, so it covers everything before itself, matching what the receiver's
		// checkChecksum() computes at the same point. Both are forced-width encodings
		// (writeIntU8/writeIntU16, never minimised to a smaller msgpack type) so a receiver
		// never has to guess how many bytes the trailer occupies.
		msgpack::writeIntU8(cobsStream, this->lastRxSeq);
		msgpack::writeIntU16(cobsStream, cobsStream.getTxRunningCRC());
		this->endTransmission();
	}

	//---------
	void
	RS485::sendACK(bool success)
	{
		if(!this->replyAllowed()) {
			return;
		}

		this->beginTransmission();

		msgpack::writeArraySize4(cobsStream, 5);
		{
			// First element is target address (0 = Host)
			msgpack::writeIntU7(cobsStream, 0);

			// Second element is our address
			msgpack::writeIntU7(cobsStream, this->app->id->get());

			// Third element is message to send
			{
				// Value is the data to transmit
				msgpack::writeBool(cobsStream, success);
			}
		}
		this->finishFrame();
	}
}