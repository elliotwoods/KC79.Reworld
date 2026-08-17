#pragma once

#include "../msgpack-arduino/msgpack.hpp"
#include "Exception.hpp"

#include "flash.hpp"

class FWUpdateApp {
public:
	void update();
	bool isFWIncoming() const;
	Exception processIncoming(msgpack::Stream&);
private:
	uint32_t writePosition = 0;
	bool fwPacketReceivedThisFrame = false;
	bool fwPacketReceivedNextFrame = false;
};
