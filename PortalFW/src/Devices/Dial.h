#pragma once

namespace Devices {
	class Dial {
	public:
		void init() { };
		int16_t getPosition() const { return 0; };
		bool isPressed() const { return false; };
	};
}