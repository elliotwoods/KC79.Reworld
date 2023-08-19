#pragma once

#include "Base.h"

#include "HardwareSerial.h"
#include <memory>
#include <deque>

#define ID_PORTAL_MIN 1
#define ID_PORTAL_MAX 127

// Message format is 5 bytes:
// [ID, C^ID, R^ID, C^ID, 0]

namespace Modules {
	class ID : public Base{
	public:
		typedef uint8_t Value;

		ID();

		const char * getTypeName() const;
		void setup();
		void initFromBoard();
		void update();

		Value get() const;
		bool getIsIDNewThisFrame() const;
	protected:
		void readIncomingID();
		void sendIDToNext();

		static const uint32_t binaryPins[4];

		Value value = 1;
		bool isIDNewThisFrame = true;
		bool markNewID = false;
		std::deque<uint8_t> incomingBytes;

		uint32_t lastSend = 0;
	};
}
