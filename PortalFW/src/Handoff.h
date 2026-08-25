// What the application tells the bootloader on its way into it.
//
// # The problem
//
// A bootloader has no address. The RS485 id is assigned by the daisy-chain on USART3, which the
// *application* runs; a board sitting in its bootloader has never read it and has no way to. The
// fielded bootloader dealt with that by only ever accepting broadcasts and never speaking, which
// is why an update could not be steered, checked or repaired -- every board on the bus was
// addressed as one anonymous crowd.
//
// So before the application resets into the bootloader, it leaves a note: this is who I am, and
// this is what I want. Thirty-two bytes at the top of SRAM, above the stack and outside both
// images' linker RAM, which startup neither copies nor zeroes -- the one region that survives a
// reset intact.
//
// # What it buys
//
// With an id in it the bootloader can be unicast, can answer, and can be told to erase, report
// what it received and verify what it programmed. With `request = STAY` it waits thirty seconds
// for an update rather than the three it waits after an ordinary power-on, which removes the race
// the host used to have to paper over by shouting announce frames for the whole update.
//
// The CRC is not decoration. This RAM is whatever the last program left there; without it, a stale
// pattern that happened to look like a magic word would give a board a wrong bus address.
#pragma once

#include <stdint.h>

#include "portal_flash_layout.h"

namespace Handoff {

	/// Record who we are and what we want, then leave it for the bootloader to find.
	///
	/// Call immediately before `NVIC_SystemReset()`. `request` is one of
	/// `PORTAL_HANDOFF_REQUEST_NONE` (an ordinary reboot: take the id, use the short window) or
	/// `PORTAL_HANDOFF_REQUEST_STAY` (an update is coming: hold for thirty seconds).
	void write(int8_t id, uint32_t serial, uint8_t request);

	/// Whether a valid block is present. Only useful for diagnostics and tests.
	bool present();

} // namespace Handoff
