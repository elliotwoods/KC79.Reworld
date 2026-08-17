#include "../msgpack-arduino/msgpack.hpp"

extern "C" {
	#include "stm32g0xx_hal.h"
}

namespace msgpack {
	//----------
	void
	delay(uint32_t ms)
	{
		HAL_Delay(ms);
	}
}
