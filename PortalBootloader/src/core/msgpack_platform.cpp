// The symbols msgpack-arduino's non-Arduino path declares but does not define.
//
// `msgpack::delay` is the real one. `waitForData` calls it between polls of `available()` when a
// parser asks for more bytes than have arrived, and here it does nothing at all -- deliberately.
// The stream this bootloader parses from only reports bytes once a *complete* COBS frame is in
// the receive ring, so the parser can never ask for a byte that has not already arrived. A
// sleeping delay would therefore only ever be reached on a truncated frame, where it would cost
// 100 ms of a watchdog period to discover something that is already known.
//
// `msgpack::String`'s two constructors are declared in `NotArduino.hpp` and defined nowhere, on
// target as well as on the host. Their only referent is `readStringNew` (`deserialize.cpp`), which
// nothing calls and `--gc-sections` discards; a host link can still demand they exist. Defining
// them empty is inert by design -- if anything ever does call `readStringNew` it will get an empty
// string rather than something plausible-looking, which is the failure worth having.
//
// This file is compiled into both the firmware and the native tests, so neither can drift into
// linking against a different set of these than the other.

#include "msgpack/Platform.hpp"

namespace msgpack {

	void delay(uint32_t)
	{
	}

	String::String()
	{
	}

	String::String(const char *)
	{
	}

}
