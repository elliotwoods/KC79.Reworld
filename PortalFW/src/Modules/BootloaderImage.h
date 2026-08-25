// Replacing the bootloader, over RS485, from the running application.
//
// # Why this exists
//
// The bootloader occupies the first pages of flash and is the only thing that can write the rest.
// It cannot write *itself*: erasing the page you are executing from stalls the fetch that would
// have brought back the next instruction. So a bootloader has always been replaceable only with a
// debug probe, physically, one board at a time.
//
// That is affordable for a bench and not for an installation. This module is the other path: the
// application receives a bootloader image over the bus, checks it thoroughly, and then -- as the
// last thing it ever does -- erases and rewrites the bootloader bank and resets into it.
//
// # The window, stated plainly
//
// Between the first erase and the last verified write there is roughly half a second during which
// the board has no bootloader. A power loss inside it leaves a board that needs a debug probe.
// Nothing here can remove that; what it can do is make the window as short as possible and refuse
// to enter it unless everything checkable has been checked. So the whole image is received and
// validated first -- length, CRC-32C, a plausible stack pointer and reset vector, and the presence
// of the banner string that identifies it as a bootloader at all -- and only then does a single
// uninterrupted sequence erase, program, read back and reset.
//
// # What it deliberately does not do
//
// It does not touch the three durable pages, and it cannot: the erase is bounded to the pages
// below this application's own base. It does not run while a motion routine is in progress. And it
// does not verify by re-reading through the same pointer it wrote through -- the read-back compare
// is a separate pass over memory-mapped flash.
#pragma once

#include "Base.h"

#include "portal_flash_layout.h"

namespace Modules {
	class App;

	class BootloaderImage : public Base {
	public:
		BootloaderImage(App *);

		const char * getTypeName() const override;

		/// Where a transfer has got to. Reported by the `q` verb so a host can resume or abandon.
		enum class State : uint8_t {
			/// Nothing in progress.
			Idle = 0,
			/// A transfer has been declared and chunks are being accepted.
			Receiving = 1,
			/// Every chunk arrived and the image passed its checks. Only reachable between the
			/// last data frame and the commit that follows it.
			Ready = 2,
			/// The image was rejected. `q` reports it until the next `begin`.
			Rejected = 3,
		};

		bool processIncoming(Stream &);

	protected:
		bool processIncomingByKey(const char * key, Stream &) override;

		bool handleBegin(Stream &);
		bool handleData(Stream &);
		bool handleCommit(Stream &);
		void handleQuery();

		/// Everything checkable about the received image, checked.
		bool imageIsPlausible() const;

		/// Erase and rewrite the bootloader bank, then reset. Does not return.
		void install(bool stayInBootloader);

		App * app;

		State state = State::Idle;
		uint32_t declaredLength = 0;
		uint32_t declaredCrc = 0;
		uint32_t receivedBytes = 0;

		/// The image, in RAM, in full.
		///
		/// Static rather than allocated. `.data + .bss` is about 5 kB here, so this takes the
		/// application to roughly 21 kB of the part's 36 -- affordable, and the alternative is a
		/// 16 kB `malloc` succeeding or failing depending on how fragmented the heap has become
		/// after months of uptime, at the one moment when failing is most expensive.
		static uint8_t buffer[PORTAL_BOOTLOADER_BYTES];
		/// One bit per 128-byte chunk, so a host can be told what to resend.
		static uint8_t received[PORTAL_BOOTLOADER_BYTES / 128 / 8];
	};
}
