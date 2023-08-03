#pragma once

#include "Base.h"

#include "HardwareSerial.h"
#include <memory>

namespace Modules {
	class ID : public Base{
	public:
		typedef uint8_t Value;

		struct Config {
			Value defaultValue = 0;

			uint32_t binaryPins[4] {PD0, PD1, PD2, PD3};
		};

		ID(const Config&);

		void setup();
		void initFromBoard();
		void update();

		Value get() const;
		bool getIsIDNewThisFrame() const;
	protected:
		const Config config;

		Value value;
		bool isIDNewThisFrame = true;
		bool markNewID = false;
	};
}
