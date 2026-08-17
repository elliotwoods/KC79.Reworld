#include "RS485.hpp"
#include "Logger.hpp"
#include "FWUpdateApp.hpp"

//----------
RS485::RS485(SerialStream &serialStream, FWUpdateApp &fwUpdateApp) :
		serialStream(serialStream), cobsStream(serialStream), fwUpdateApp(
				fwUpdateApp) {

}

//----------
void RS485::update() {
	this->processIncoming();
}

//----------
void RS485::processIncoming() {
	// Skip any partial packets
	if (!this->cobsStream.isStartOfIncomingPacket()) {
		this->cobsStream.nextIncomingPacket();
	}

	while (this->cobsStream.available()
			&& this->cobsStream.isStartOfIncomingPacket()) {
		auto exception = this->processCOBSPacket();
		if (exception) {
			log(LogLevel::Error, exception.what());
		}

		cobsStream.nextIncomingPacket();
	}
}

//----------
Exception RS485::processCOBSPacket() {
	// Packet should be a 3-element array
	{
		size_t arraySize;
		if (!msgpack::readArraySize(cobsStream, arraySize)) {
			return Exception::MessageFormatError();
		}
		;
		if (arraySize < 3) {
			return Exception::MessageFormatError();
		}
	}

	// Expecting a broadcast packet
	// First element is target address
	{
		int32_t targetAddress;
		if (!msgpack::readInt<int32_t>(cobsStream, targetAddress)) {
			return Exception::MessageFormatError();
		}

		// An address of -1 means it's addressed to all devices
		// If it's not a broadcast, then we ignore it
		if (targetAddress != -1) {
			log(LogLevel::Status, "Non-broadcast packet");
			return Exception::None();
		}
	}

	// Second element is the source address (we ignore)
	{
		uint8_t _;
		if (!msgpack::readInt<uint8_t>(cobsStream, _)) {
			return Exception::MessageFormatError();
		}
	}

	// Send remainder of packet to the FWUpdateApp
	{
		auto exception = this->fwUpdateApp.processIncoming(cobsStream);
		if(exception) {
			log(LogLevel::Error, exception.what());
		}
	}

	return Exception::None();
}
