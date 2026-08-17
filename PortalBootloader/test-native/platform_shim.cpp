// The symbols msgpack-arduino's non-Arduino path declares but does not define.
//
// `msgpack::delay` is the real one: on target, BootloaderRS485's Core/Src/msgpack_HAL.cpp
// supplies exactly this, forwarding to HAL_Delay. Here it is a no-op, because the tests drive a
// loopback stream where every byte is already present and nothing ever waits.
//
// `msgpack::String`'s two constructors are declared in NotArduino.hpp and defined nowhere -- on
// target as well as here. The firmware links anyway because their only referent is
// readStringNew() (deserialize.cpp:560), which nothing calls, and CubeIDE's -flto plus
// -ffunction-sections/--gc-sections drops it before the linker ever asks. MSVC resolves symbols
// before /OPT:REF runs, so the host link needs them to exist. Defining them empty here is
// deliberately inert: no test calls readStringNew, and if one ever does it will return an empty
// String rather than something plausible-looking, which is the failure you want.

#include <msgpack.hpp>

namespace msgpack {
	void delay(uint32_t)
	{
	}

	String::String()
	{
	}

	String::String(const char*)
	{
	}
}
