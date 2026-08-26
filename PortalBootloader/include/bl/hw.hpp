// The only place the bootloader touches hardware.
//
// Everything above this line -- the frame parser, the upload session, the run decision, the state
// machine -- is ordinary C++ that runs on a laptop. That is not tidiness for its own sake: this
// firmware can only be replaced with a debug probe, so the parts of it that decide whether to
// erase flash or jump to an address need to be exercised by a test suite that can run thousands of
// cases in a second, not by a board on a bench that is in use.
//
// So the seam is drawn here, and it is drawn at *free functions* rather than at an abstract class.
// A vtable would cost a pointer indirection on every flash write and a relocation table entry per
// method, in an image with a hard size limit; two implementations of the same free functions cost
// nothing and are selected by which .cpp the build compiles.
//
// `src/target/hw_stm32.cpp` implements these against the STM32G070. `lib/bltest/fake_hw.cpp`
// implements them against a 128 kB array, a settable clock, and a recorded list of terminal
// actions.
#pragma once

#include <stdint.h>
#include <stddef.h>

#include "bl/config.hpp"

namespace bl {
namespace hw {

	// ---- Flash -----------------------------------------------------------------------------

	/// Erase one 2 kB page. Returns 0 on success, or the flash controller's error bits.
	///
	/// On the target this runs from RAM: erasing stalls every flash read for tens of milliseconds,
	/// so code that lives in flash cannot execute during it -- including, crucially, the UART
	/// receive interrupt. Losing incoming bytes for 25 ms per page across a 53-page erase is what
	/// made the v4/v5 bootloader unable to hear anything for over a second after `ER`, which is
	/// why the host had to blanket the erase in three seconds of announce frames and hope.
	uint32_t flashErasePage(uint32_t page);

	/// Program one 8-byte double-word. Returns 0 on success, or the flash controller's error bits.
	///
	/// The target may only program an *erased* double-word; reprogramming one raises PROGERR even
	/// when the new value is identical. The caller checks, rather than this function, because the
	/// caller is the one that knows whether a repeat is a duplicate frame (fine) or a bug.
	uint32_t flashProgram8(uint32_t address, const uint8_t * data);

	/// A readable pointer to flash at `address`.
	///
	/// On the target this is the address itself: flash is memory-mapped. In tests it is an offset
	/// into an array. Every read of programmed data goes through here so the core never contains a
	/// dereference of a literal address, which is what would otherwise make it untestable.
	const uint8_t * flashPtr(uint32_t address);

	// ---- Identity --------------------------------------------------------------------------

	/// The MCU's 96-bit unique id.
	void uid(uint32_t out[3]);

	/// The four ID DIP switches, PD0-PD3, as 0..15. Active low.
	///
	/// The last-resort address source. The application maps this to `value + 1`; so does the
	/// bootloader, so a board answers on the same address in both.
	uint8_t dip();

	// ---- The handoff block -------------------------------------------------------------------

	/// The 32 bytes of SRAM the application writes before resetting into the bootloader.
	///
	/// Returns a pointer rather than a copy because the bootloader writes to it too: it clears the
	/// request after acting on it, and stores an adopted id there so a later watchdog reset does
	/// not lose it.
	portal_handoff_t * handoff();

	// ---- Time and the watchdog ----------------------------------------------------------------

	/// Milliseconds since reset.
	uint32_t millis();

	/// Reload the independent watchdog.
	///
	/// The IWDG cannot be stopped once started, and it is started by this bootloader with a ~4.1 s
	/// period. Anything that blocks for longer than that -- a 53-page erase, a 108 kB CRC -- has to
	/// call this from inside its loop, not around it.
	void watchdogKick();

	/// Hold off before transmitting a reply, so the far end can release the bus.
	///
	/// See `config::replyGuardMs` for what it is worth and why. A no-op on the host, where there
	/// is no bus and no clock running of its own accord.
	void replyGuard();

	/// Whether the UART receive ring has overrun since this was last asked, clearing the flag.
	///
	/// Clear-on-read is the ISR's contract, not this one's: the caller is expected to latch the
	/// answer somewhere that outlives a single question, because "did this board ever fall behind"
	/// is the useful form of it. It is one of only two explanations for an upload that keeps
	/// losing frames -- the other being a frame too long for the window -- and until it was
	/// reported, neither could be asked about from the bus.
	bool ringOverran();

	// ---- Indicators ---------------------------------------------------------------------------

	enum class Led : uint8_t { Frame = 0, Heartbeat = 1 };

	void ledToggle(Led led);
	void ledSet(Led led, bool on);

	/// One character to the debug UART (USART1, the ST-Link VCOM).
	void logChar(char value);
	/// A NUL-terminated string to the debug UART.
	void logString(const char * text);

	// ---- Terminal actions ---------------------------------------------------------------------

	/// Block until the RS485 transmitter has finished.
	///
	/// Called before any reset or jump. The DE line drops when the shift register empties, so
	/// resetting a microsecond early truncates the reply the host is waiting for -- and on a
	/// half-duplex bus a truncated frame is not just a lost reply, it is a lost turnaround.
	void txDrain();

	/// Reset the MCU. Does not return on the target.
	void reset();

	/// Start the application at `base`. Does not return on the target.
	///
	/// Implemented as "write the handoff and reset" rather than as an in-place jump: after a reset
	/// every peripheral is already in the state the application's own initialisation expects,
	/// which removes the whole class of failure where a half-torn-down clock tree makes the
	/// application's `HAL_RCC_OscConfig` refuse a PLL that is currently driving SYSCLK.
	void runApplication(uint32_t base);

	/// Whether `reset`/`runApplication` actually end execution.
	///
	/// True on the target. False in tests, where they record what they were asked to do and
	/// return, so a test can assert on the decision rather than on a process exiting.
	bool terminalActionsHalt();

} // namespace hw
} // namespace bl
