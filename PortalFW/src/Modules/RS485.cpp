#include "RS485.h"
#include <Arduino.h>

#include <msgpack.hpp>

#include "App.h"
#include "Logger.h"
#include "Exception.h"

HardwareSerial serialRS485(PA3, PA2);
msgpack::COBSRWStream cobsStream(serialRS485);

#define BOOTLOADER_FLASH_ADDRESS 0x08000000U

void run_bootloader()
{
	typedef void (*bootloader_reset_handler_pointer)(void);
	__IO uint32_t* bootloader_vect_table = (__IO uint32_t*) BOOTLOADER_FLASH_ADDRESS;
	bootloader_reset_handler_pointer bootloader_reset_handler = (bootloader_reset_handler_pointer) *(bootloader_vect_table + 1);

	HAL_RCC_DeInit();
	HAL_DeInit();

	SysTick->CTRL = 0;
	SysTick->LOAD = 0;
	SysTick->VAL  = 0;

	SCB->VTOR = (uint32_t) bootloader_vect_table;

	__set_MSP(*bootloader_vect_table);

	bootloader_reset_handler();
}

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
		
		// Packer [target, sender, message]
		msgpack::writeArraySize4(cobsStream, 3);
		{
			msgpack::writeInt8(cobsStream, 0);
			msgpack::writeInt8(cobsStream, ourID);

			// From here we use Serializer
			msgpack::Serializer serializer(cobsStream);
			this->app->reportStatus(serializer);
		}

		this->endTransmission();
	}

	//---------
	void
	RS485::sendPositions()
	{
		this->beginTransmission();

		const auto ourID = this->app->id->get();
		
		// Packer [target, sender, message]
		msgpack::writeArraySize4(cobsStream, 3);
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

		this->endTransmission();
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
		RS485::instance->sentACKEarly = true;
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
			bool packetNeedsACK = false;

			auto exception = this->processCOBSPacket(packetNeedsACK);
			if(exception) {
				log(LogLevel::Error, exception.what());
			}

			if(packetNeedsACK) {
				if(exception) {
					this->sendACK(false);
				}
				else {
					log(LogLevel::Status, "RS485 Rx");
					this->sendACK(true);
				}
			}

			cobsStream.nextIncomingPacket();
		}
	}

	//---------
	Exception
	RS485::processCOBSPacket(bool & packetNeedsACK)
	{
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
					return Exception::MessageFormatError();
				};
				if(arraySize < 3) {
					return Exception::MessageFormatError();
				}
			}

			// First element is target address
			int8_t targetAddress;
			{
				if(!msgpack::readInt<int8_t>(cobsStream, targetAddress)) {
					return Exception::MessageFormatError();
				}

				if(targetAddress == this->app->id->get()) {
 					weShouldProcess = true;
					packetNeedsACK = true;
				}

				// An address of -1 means it's addressed to all devices
				// We process but we don't ACK
				if(targetAddress == -1) {
 					weShouldProcess = true;
				}
			}

			// Second element is the source address (we ignore)
			{
				int8_t _;
				if(!msgpack::readInt<int8_t>(cobsStream, _)) {
					return Exception::MessageFormatError();
				}
			}

			if(weShouldProcess) {
				// We should process this packet
				// There are different types of packet:

				if(msgpack::nextDataTypeIs(cobsStream, msgpack::DataType::Nil)) {
					// If it's a Nil, then it's a ping
					if(!msgpack::readNil(cobsStream)) {
						return Exception::MessageFormatError();
					}
					// Will result in an ACK being sent (the ping reply)
				}
				else if(msgpack::nextDataTypeIs(cobsStream, msgpack::DataType::String5)) {
					// If it's a string5, it's a magic word
					char word[64];
					uint8_t wordSize;
					if(!msgpack::readString5(cobsStream, word, 64, wordSize)) {
						return Exception::MessageFormatError();
					}
					if(word[0] == 'F' && word[1] == 'W') {
						// Firmware announce packet
						// Reset into the bootloader
						log(LogLevel::Status, "Firmware announced, rebooting...");
						HAL_Delay(500);
						NVIC_SystemReset();
					}
				}
				else {
					// If it's a map, it's a message for the app
					auto success = app->processIncoming(cobsStream);
					if(!success) {
						return Exception::MessageFormatError();
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
	RS485::sendACK(bool success)
	{
		if(this->disableACK || this->sentACKEarly) {
			return;
		}

		this->beginTransmission();

		msgpack::writeArraySize4(cobsStream, 3);
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
		this->endTransmission();
	}
}