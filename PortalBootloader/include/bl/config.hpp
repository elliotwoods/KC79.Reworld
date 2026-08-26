// Everything the bootloader is configured by, in one place.
//
// The flash and RAM addresses come from `portal_flash_layout.h`, which the application, the host
// tooling and the test suites all read too. What is added here is the arithmetic those raw
// addresses imply (page numbers, bank sizes, granule counts) and the timings, which are the
// bootloader's own business and nobody else's.
#pragma once

#include <stdint.h>
#include <stddef.h>

#include "portal_flash_layout.h"

namespace bl {
namespace config {

	// ---- Identity ------------------------------------------------------------------------

	// Reported by `status` as `v`. The host uses it to decide whether it can use the addressed
	// protocol at all, so it is a capability gate rather than a decoration.
	constexpr uint8_t protocolVersion = PORTAL_BL_PROTO_VERSION;

	// The banner, printed once at startup on the debug UART.
	//
	// It is also how a board is identified over SWD: `portal-swd` scrapes "Bootloader v" out of a
	// flash readback (`device.rs`, BANNER_NEEDLES) because a string literal in .rodata is the one
	// piece of version information that needs no firmware cooperation to read. Keep the literal
	// referenced, and keep the prefix.
	constexpr const char * banner = "Bootloader v6";

	// ---- Flash geometry ------------------------------------------------------------------

	constexpr uint32_t flashBase = PORTAL_FLASH_BASE;
	constexpr uint32_t pageBytes = PORTAL_FLASH_PAGE_BYTES;

	constexpr uint32_t bootBank = PORTAL_BOOTLOADER_BYTES;
	constexpr uint32_t appBase = PORTAL_APP_BASE;
	constexpr uint32_t appBaseLegacy = PORTAL_APP_BASE_LEGACY;
	constexpr uint32_t appEnd = PORTAL_APP_END;

	// The bank a v6 bootloader owns: pages 8..60 inclusive.
	constexpr uint32_t appCap = appEnd - appBase;
	constexpr uint32_t appFirstPage = (appBase - flashBase) / pageBytes;
	constexpr uint32_t appPageCount = appCap / pageBytes;
	constexpr uint32_t appLastPage = appFirstPage + appPageCount - 1;

	// What an image at the legacy base can occupy. Smaller, because it starts 8 kB higher.
	constexpr uint32_t appCapLegacy = appEnd - appBaseLegacy;

	// Flash programs a double-word at a time, so this is the smallest unit that can be written,
	// and therefore the unit the received-data bitmap counts in.
	constexpr uint32_t granule = PORTAL_FLASH_GRANULE;
	constexpr uint32_t granuleCount = appCap / granule;
	constexpr uint32_t bitmapBytes = (granuleCount + 7u) / 8u;

	constexpr uint32_t ramBase = PORTAL_RAM_BASE;
	constexpr uint32_t ramEnd = PORTAL_RAM_END;

	// ---- Frames --------------------------------------------------------------------------

	// Largest data payload accepted in one frame.
	//
	// Bounded by the msgpack library's decode buffer (MSGPACK_COBSRWSTREAM_BUFFER_SIZE, 256), not
	// by anything on the host side. A fielded v4/v5 bootloader carries a 64-byte buffer instead,
	// which is why the legacy host sends 32.
	constexpr size_t chunkMax = PORTAL_BL_CHUNK_MAX;
	// The 16-bit XOR checksum that prefixes every data payload.
	constexpr size_t payloadChecksumBytes = 2;
	// One static receive buffer, sized for the largest legitimate frame. Nothing here is a VLA:
	// the v4 bootloader sized a stack array from an unbounded 16-bit wire value, which a corrupt
	// frame could use to smash the stack of an image only recoverable with an ST-Link.
	constexpr size_t payloadBufferBytes = chunkMax + payloadChecksumBytes;

	// Longest verb or key string read off the wire. "status" is 6.
	constexpr size_t wordBufferBytes = 32;

	// One whole COBS frame, buffered so the codec never sees past its delimiter.
	//
	// The largest legitimate frame is a data payload: a 5-element envelope, a one-entry map keyed
	// by a uint32 offset, a bin8 header, `chunkMax` bytes plus their checksum, and the trailer.
	// COBS adds one byte per 254, and the delimiter is one more. Rounded up generously -- this is
	// .bss in a part with 36 kB of it, and an undersized frame buffer would show up as an upload
	// that fails only on its largest frames.
	constexpr size_t frameBufferBytes = 320;

	// ---- Timing (milliseconds) -------------------------------------------------------------

	// How long a board stays in the bootloader after a plain reset, before starting the
	// application. This is the legacy window, and it is short because on a healthy board every
	// power-on pays it.
	constexpr uint32_t residencyDefault = 3000;
	// ...and after the application deliberately reset into the bootloader, which means an update
	// is expected and there is no reason to hurry.
	constexpr uint32_t residencyHandoff = 30000;
	// Any accepted frame pushes the deadline out at least this far, so a slow host cannot lose a
	// board mid-conversation.
	constexpr uint32_t residencyExtend = 10000;
	// Once a session is open the board waits this long between frames before giving up on it.
	// Long, because the host may be servicing 53 other boards in between.
	constexpr uint32_t sessionSilence = 60000;

	// Heartbeat LED half-periods, chosen so the three states are distinguishable across a rack:
	// a slow blink while idle, a fast one while receiving, a very fast one when there is nothing
	// valid to run.
	//
	// The values used to say the opposite of that sentence -- idle blinked fastest and receiving
	// slowest. Nothing had ever watched them, because this firmware had not run on a board; the
	// comment describes the intent and the conventional reading, so the numbers were brought to
	// it rather than the other way round.
	constexpr uint32_t heartbeatIdle = 500;
	constexpr uint32_t heartbeatBusy = 100;
	constexpr uint32_t heartbeatNoApp = 50;

	// ---- Sanity ----------------------------------------------------------------------------

	static_assert(appCap == 108544, "the v6 application bank is 53 pages");
	static_assert(appPageCount == 53, "the v6 application bank is 53 pages");
	static_assert(appFirstPage == 8, "the v6 bootloader occupies pages 0-7");
	static_assert(appLastPage == 60, "the durable pages start at page 61");
	static_assert(appCap % pageBytes == 0, "the bank must be a whole number of erasable pages");
	static_assert(appCap % granule == 0, "the bank must be a whole number of programmable units");
	static_assert(bitmapBytes == 1696, "the granule bitmap is 1,696 bytes of .bss");
	static_assert(chunkMax % granule == 0, "a chunk must be a whole number of double-words");
	static_assert(appBase + appCap == appEnd, "the bank cannot reach the durable pages");

} // namespace config
} // namespace bl
