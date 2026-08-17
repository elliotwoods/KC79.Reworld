#pragma once

#include "SerialStream.hpp"
#include "../msgpack-arduino/msgpack.hpp"
#include "Exception.hpp"
#include "FWUpdateApp.hpp"

class RS485 {
public:
	RS485(SerialStream&, FWUpdateApp&);
	void update();
protected:
	void processIncoming();
	SerialStream & serialStream;
	msgpack::COBSRWStream cobsStream;
	FWUpdateApp& fwUpdateApp;

	Exception processCOBSPacket();
};
