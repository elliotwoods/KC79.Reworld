#pragma once

#include "Base.h"

#include "HardwareSerial.h"
#include <memory>

#define ID_PORTAL_MIN 1
#define ID_PORTAL_MAX 127

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
		static const uint32_t binaryPins[4];

		Value value = 1;
		bool isIDNewThisFrame = true;
		bool markNewID = false;
	};
}
