#pragma once

#include "ofMain.h"

#define BAUD_RATE 115200

namespace SerialDevices {
	// A messagepack encoded message (not COBS yet)
	typedef vector<uint8_t> Buffer;

	class IDevice
	{
	public:
		virtual string getTypeName() const = 0;
		
		virtual bool open(const nlohmann::json&) = 0;
		virtual void close() = 0;

		virtual size_t transmit(const Buffer&) = 0;

		virtual bool hasDataIncoming() = 0;

		// Note that messages may be partial and need packetising
		virtual Buffer receiveBytes() = 0;
	};
}