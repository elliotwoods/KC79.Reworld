#include "FWUpdateApp.hpp"
#include "Logger.hpp"
#include <stdio.h>
#include "constants.h"

extern "C" {
	#include "RunApplication.h"
}


// size in bytes

typedef uint16_t CRCType;

//----------
void
FWUpdateApp::update()
{
	this->fwPacketReceivedThisFrame = this->fwPacketReceivedNextFrame;
	this->fwPacketReceivedNextFrame = false;
}

//----------
bool
FWUpdateApp::isFWIncoming() const
{
	return this->fwPacketReceivedThisFrame;
}

//----------
CRCType calcCheckSum(uint8_t* data, uint32_t size)
{
	CRCType value = 0;
	auto wordPosition = (CRCType*)data;
	auto dataEnd = (CRCType*)(data + size);

	while (wordPosition < dataEnd) {
		value ^= *wordPosition++;
	}

	return value;
}

//----------
Exception
FWUpdateApp::processIncoming(msgpack::Stream& stream)
{
	if(stream.available() == 0) {
		return Exception::None();
	}

	if(msgpack::nextDataTypeIs(stream, msgpack::DataType::String5)) {
		// Maybe the FW announce packet
		uint8_t allocatedSize = 3;
		uint8_t outputSize = 2;
		char message[allocatedSize];
		if(!msgpack::readString5(stream, message, allocatedSize, outputSize, true)) {
			return Exception::MessageFormatError();
		}
		if(message[0] == 'F' && message[1] == 'W') {
			// FIRMWARE ANNOUNCE

			// It's a firmware announcement packet
			this->fwPacketReceivedNextFrame = true;

			// Reset the write position (e.g. if previous upload failed and we got to here)
			this->writePosition = 0;

			return Exception::None();
		}
		else if(message[0] == 'E' && message[1] == 'R') {
			// ERASE

			return flash_erase();
		}
		else if(message[0] == 'R' && message[1] == 'U') {
			// RUN

			run_application();
		}
		return Exception::MessageFormatError();
	}
	else if(msgpack::nextDataTypeIs(stream, msgpack::DataType::Map)){
		// Maybe a FW packet { packetIndex : packetData }

		// Map of [frameOffset -> data]
		{
			size_t _;
			if(!msgpack::readMapSize(stream, _, true)) {
				return Exception::MessageFormatError();
			}
		}

		// Key is frameOffset
		uint32_t frameOffset;
		if(!msgpack::readInt(stream, frameOffset, true)) {
			return Exception::MessageFormatError();
		}

		// Value is blob of binary data. The first 2 bytes are the checksum
		{
			uint16_t packetBodyAndChecksumSize16;
			if(!msgpack::readBinarySize(stream, packetBodyAndChecksumSize16, true)) {
				return Exception::MessageFormatError();
			}
			size_t packetBodyAndChecksumSize = packetBodyAndChecksumSize16;
			size_t checksumSize = sizeof(CRCType);
			size_t packetBodySize = packetBodyAndChecksumSize - checksumSize;

			uint8_t dataWithChecksum[packetBodyAndChecksumSize];
			uint8_t * dataBody = dataWithChecksum + checksumSize;

			if(!msgpack::readRaw(stream
					, (char*) dataWithChecksum
					, packetBodyAndChecksumSize
					, true)) {
				return Exception::MessageFormatError();
			}

			// Check the checksum
			{
				auto checkSumCalculated = calcCheckSum(dataBody, packetBodyAndChecksumSize - checksumSize);
				auto checkSumTransmitted = * (CRCType*) dataWithChecksum;
				if(checkSumCalculated != checkSumTransmitted) {
					return Exception("FW : Checksum FAIL");
				}
			}

			// Check that position is moving continuously
			if(frameOffset < writePosition) {
				logPrint("R"); // R for repeated
				return Exception::None();
			}
			if(frameOffset > writePosition) {
				log(LogLevel::Error, "FW : Write position is ahead of ours");
				return Exception("Non-continuous packet");
			}

			// Perform the write
			flash_write(dataBody
				, (uint32_t)APP_FLASH_ADDRESS + (uint32_t)frameOffset
				, packetBodySize);

			// Flash the indicator
			HAL_GPIO_TogglePin(GPIOB, GPIO_PIN_3);

			// Avoid changing any code inside this routine
//			// Disable the other indicator
//			HAL_GPIO_WritePin(GPIOB, GPIO_PIN_4, GPIO_PinState::GPIO_PIN_RESET);

			// Notify it's OK
			logPrint("O"); // O for OK

			this->fwPacketReceivedNextFrame = true;
			this->writePosition += (uint32_t) packetBodySize;
		}

		return Exception::None();
	}
	else {
		msgpack::DataType dataType;
		auto data = stream.peek();
		msgpack::getNextDataType(stream, dataType, true);
		return Exception::MessageFormatError();
	}
}
