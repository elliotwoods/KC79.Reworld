// What the bootloader can refuse to do, and why.
//
// Numeric because this image has no `printf` and no room for one: the two `sprintf` calls in the
// v4/v5 `flash.cpp` pulled in roughly 1.9 kB of newlib formatting machinery for two diagnostics
// nobody could read anyway, and both of them returned a pointer to a stack buffer that had already
// gone out of scope. The host holds the strings, in `router_proto::bootloader::error_name`.
//
// The values are wire format. They must not be renumbered.
#pragma once

#include <stdint.h>

namespace bl {

	enum class Error : uint8_t {
		None = 0,
		/// The frame did not parse, or a field had the wrong type.
		Format = 1,
		/// The frame's trailing CRC-16 did not match: it was corrupted in flight.
		Crc16 = 2,
		/// An offset or length reached outside the application bank.
		Bounds = 3,
		/// An offset was not a multiple of the 8-byte programming granule.
		Align = 4,
		/// A data payload's own XOR-16 checksum did not match.
		Xor = 5,
		/// Flash programming failed, or the target double-word was not erased.
		Program = 6,
		/// Flash erase failed.
		Erase = 7,
		/// Busy erasing; try again when `status` says otherwise.
		Busy = 8,
		/// There is no valid application to start.
		NoApp = 9,
		/// The installed image carries no application descriptor.
		DescriptorMissing = 10,
		/// The installed image says it was linked for a different base address.
		DescriptorBase = 11,
		/// The programmed image does not match the CRC-32C that `begin` declared.
		ImageCrc = 12,
		/// A `bl` verb this bootloader does not implement.
		UnknownVerb = 13,
		/// A broadcast verb that needs a selector did not carry one.
		SelectorRequired = 14,
		/// A parameter was outside its permitted range.
		BadParam = 15,
	};

	constexpr bool failed(Error error)
	{
		return error != Error::None;
	}

	constexpr uint8_t code(Error error)
	{
		return static_cast<uint8_t>(error);
	}

	/// A single character per error, for the debug UART.
	///
	/// The whole log is single characters: at 115200 baud, on a board being driven by a host that
	/// is talking to 53 others, a sentence per event would take longer to send than the events
	/// take to happen.
	constexpr char marker(Error error)
	{
		return error == Error::None ? 'O'
			: error == Error::Crc16 ? 'c'
			: error == Error::Bounds ? 'b'
			: error == Error::Align ? 'a'
			: error == Error::Xor ? 'x'
			: error == Error::Program ? 'p'
			: error == Error::Erase ? 'e'
			: error == Error::Busy ? 'B'
			: 'f';
	}

} // namespace bl
