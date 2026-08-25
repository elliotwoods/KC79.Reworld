// The bootloader itself: what state it is in, how long it stays there, and what each frame does.
//
// # How long a board waits
//
// The v4/v5 bootloader stayed resident for 3 seconds after every reset and had no way to be told
// otherwise, so the host's only means of holding a fleet in its bootloaders was to keep sending
// announce frames at 100 ms intervals for the entire update. Miss the window on one board and it
// silently ran its application again while the host kept talking to it.
//
// Here the application says what it wants when it resets: `PORTAL_HANDOFF_REQUEST_STAY` means an
// update is expected, and the board waits 30 seconds rather than 3. Any accepted frame pushes the
// deadline out further, and an open session removes it entirely until the host says `run` or goes
// quiet for a minute. A board with nothing valid to start waits forever, because the alternative
// is a boot loop that looks identical to a hardware fault.
#pragma once

#include <stdint.h>

#include "bl/config.hpp"
#include "bl/errors.hpp"
#include "bl/identity.hpp"
#include "bl/image.hpp"
#include "bl/link.hpp"
#include "bl/session.hpp"

namespace bl {

	enum class Phase : uint8_t {
		/// Counting down to starting the application.
		Idle = 0,
		/// Erasing the application bank, one page per tick.
		Erasing = 1,
		/// A session is open and data frames are being accepted.
		Receiving = 2,
		/// Resident with no deadline: there is nothing valid to run.
		Held = 3,
	};

	/// Where the RS485 address came from.
	enum class IdSource : uint8_t { None, Handoff, Adopt, Dip };

	class Bootloader {
	public:
		explicit Bootloader(msgpack::Stream & io);

		/// Read the handoff block and the identity page, choose an address, print the banner.
		void begin(uint32_t now);

		/// One pass. Never blocks for longer than a single flash page erase.
		void tick(uint32_t now);

		// Observable state, for tests.
		Phase phase() const { return this->currentPhase; }
		int8_t address() const { return this->myId; }
		IdSource idSource() const { return this->source; }
		Error lastError() const { return this->recentError; }
		uint32_t deadline() const { return this->expiry; }
		bool indefinite() const { return this->noDeadline; }
		const Session & session() const { return this->upload; }

	private:
		void handle(const Command & command, uint32_t now);
		void handleControl(const Command & command, uint32_t now);
		void replyStatus(const Command & command);
		void replyError(const Command & command, const char * verb, Error error);
		void extend(uint32_t now, uint32_t by);
		void tryRun(const Command * command);
		void heartbeat(uint32_t now);
		void consumeHandoff();
		void adopt(int8_t id, IdSource from);

		Link link;
		Session upload;
		Identity identity;

		Phase currentPhase = Phase::Idle;
		IdSource source = IdSource::None;
		Error recentError = Error::None;

		int8_t myId = 0;
		uint32_t uid[3] = {0, 0, 0};

		uint32_t expiry = 0;
		bool noDeadline = false;

		uint32_t lastHeartbeat = 0;
		uint32_t lastIdleTick = 0;

		/// A `begin` cannot be answered until its erase finishes, which is a second away. The
		/// request is parked here so the reply carries the right sequence number when it does.
		bool beginPending = false;
		Command beginRequest;
	};

} // namespace bl
