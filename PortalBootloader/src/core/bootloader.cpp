#include "bl/bootloader.hpp"
#include "bl/hw.hpp"

#include "portal_crc32c.h"

#include <string.h>

namespace bl {
	namespace {
		const char * sourceName(IdSource source) {
			switch(source) {
			case IdSource::Handoff: return "handoff";
			case IdSource::Adopt: return "adopt";
			case IdSource::Dip: return "dip";
			default: return "none";
			}
		}

		/// Recompute and store the handoff block's CRC after changing a field.
		void reseal(portal_handoff_t * block) {
			block->crc32c = portal_crc32c((const uint8_t *) block,
				(uint32_t) offsetof(portal_handoff_t, crc32c));
		}

		bool handoffValid(const portal_handoff_t * block) {
			return block->magic == PORTAL_HANDOFF_MAGIC
				&& block->version == PORTAL_HANDOFF_VERSION
				&& block->crc32c == portal_crc32c((const uint8_t *) block,
					(uint32_t) offsetof(portal_handoff_t, crc32c));
		}
	}

	//----------
	Bootloader::Bootloader(msgpack::Stream & io)
	: link(io)
	{
	}

	//----------
	void
	Bootloader::consumeHandoff()
	{
		portal_handoff_t * block = hw::handoff();
		if(!handoffValid(block)) {
			return;
		}

		if(block->id > 0) {
			this->myId = block->id;
			this->source = IdSource::Handoff;
		}

		const bool stay = (block->request == PORTAL_HANDOFF_REQUEST_STAY);

		// Clear the request but keep the identity. A watchdog reset during an update would
		// otherwise lose the address the application gave us, and there would be no application
		// left to ask again.
		block->request = PORTAL_HANDOFF_REQUEST_NONE;
		reseal(block);

		if(stay) {
			this->expiry = config::residencyHandoff;
			hw::logChar('H');
		}
	}

	//----------
	void
	Bootloader::adopt(int8_t id, IdSource from)
	{
		this->myId = id;
		this->source = from;
		this->link.setAddress(id);

		// Record it, creating the block if there is not already a valid one.
		//
		// A board being addressed by `adopt` is by definition one whose id nobody knew -- it came
		// up with no application to leave a handoff behind. If the id lived only in RAM variables,
		// the watchdog reset that an interrupted update ends in would lose it again, and the host
		// would have to rediscover the board by serial every single time.
		portal_handoff_t * block = hw::handoff();
		if(!handoffValid(block)) {
			memset(block, 0, sizeof(*block));
			block->magic = PORTAL_HANDOFF_MAGIC;
			block->version = PORTAL_HANDOFF_VERSION;
		}
		block->id = id;
		reseal(block);
	}

	//----------
	void
	Bootloader::begin(uint32_t now)
	{
		hw::uid(this->uid);

		// The default window, possibly replaced by the handoff below.
		this->expiry = config::residencyDefault;
		this->consumeHandoff();
		this->expiry += now;

		if(this->source == IdSource::None) {
			// The DIP switches, mapped exactly as the application maps them so a board answers on
			// the same address whichever image is running.
			this->myId = (int8_t) (hw::dip() + 1);
			this->source = IdSource::Dip;
		}

		this->identity = readIdentity();
		this->link.setAddress(this->myId);
		this->link.setIdentity(this->identity.valid ? this->identity.serial : 0, this->uid);

		hw::logString("\r\n");
		hw::logString(config::banner);
		hw::logString("\r\n");

		this->lastHeartbeat = now;
		this->lastIdleTick = now;
	}

	//----------
	void
	Bootloader::extend(uint32_t now, uint32_t by)
	{
		const uint32_t candidate = now + by;
		// Never brings a deadline *closer*. A short extension arriving after a long one would
		// otherwise cut a session short.
		if((int32_t) (candidate - this->expiry) > 0) {
			this->expiry = candidate;
		}
	}

	//----------
	void
	Bootloader::heartbeat(uint32_t now)
	{
		uint32_t period = config::heartbeatIdle;
		if(this->currentPhase == Phase::Erasing || this->currentPhase == Phase::Receiving) {
			period = config::heartbeatBusy;
		}
		else if(this->noDeadline) {
			period = config::heartbeatNoApp;
		}

		if(now - this->lastHeartbeat >= period) {
			this->lastHeartbeat = now;
			hw::ledToggle(hw::Led::Heartbeat);
		}
	}

	//----------
	void
	Bootloader::tick(uint32_t now)
	{
		// One page per pass. The erase is the only thing here that blocks for a measurable time,
		// and splitting it is what lets frames keep arriving through it.
		if(this->currentPhase == Phase::Erasing) {
			if(this->upload.eraseStep()) {
				const bool erased = !this->upload.eraseFailed();
				this->currentPhase = Phase::Receiving;
				this->noDeadline = false;
				this->expiry = now + config::sessionSilence;
				hw::logChar(erased ? 'e' : 'x');

				// A bank that would not erase is reported, not glossed over. Answering `ok` here
				// would send the host off to stream a whole image into flash that cannot take it,
				// and the first sign of trouble would be a `verify` mismatch with no cause.
				if(!erased) {
					this->recentError = Error::Erase;
				}

				if(this->beginPending) {
					this->beginPending = false;
					if(this->beginRequest.replyAllowed) {
						this->link.beginReply(this->beginRequest, "begin", erased ? 1 : 2);
						this->link.fieldBool("ok", erased);
						if(!erased) {
							this->link.fieldUint("err", code(Error::Erase));
						}
						this->link.endReply(this->beginRequest);
					}
				}
			}
			this->heartbeat(now);
			return;
		}

		if(this->link.pending()) {
			const Command command = this->link.receive();
			if(command.kind != CommandKind::None) {
				this->handle(command, now);
			}
		}

		this->heartbeat(now);

		if(!this->noDeadline && (int32_t) (now - this->expiry) >= 0) {
			this->tryRun(nullptr);
		}
		else if(this->currentPhase == Phase::Idle && now - this->lastIdleTick >= 100) {
			this->lastIdleTick = now;
			hw::logChar('.');
		}
	}

	//----------
	void
	Bootloader::handle(const Command & command, uint32_t now)
	{
		if(command.kind == CommandKind::Rejected) {
			this->recentError = command.error;
			hw::logChar(marker(command.error));
			return;
		}

		// Anything we understood keeps the board resident, whatever it was.
		this->extend(now, config::residencyExtend);

		switch(command.kind) {
		case CommandKind::Announce:
			// A keepalive, and nothing more. The v4/v5 bootloader also reset its write position
			// here, which meant the announce frames a host sends to cover an erase would silently
			// discard the progress of an upload already under way.
			hw::logChar('_');
			break;

		case CommandKind::Erase:
			// Legacy `ER` writes at the legacy base: a host old enough to send this word is old
			// enough to be sending an image linked for 0x08006000.
			this->upload.beginErase(config::appBaseLegacy);
			this->currentPhase = Phase::Erasing;
			this->noDeadline = false;
			hw::logChar('E');
			break;

		case CommandKind::RunLegacy:
			this->tryRun(nullptr);
			break;

		case CommandKind::Data: {
			if(this->currentPhase == Phase::Idle) {
				// A host that skipped the erase. The write will fail on the first unerased
				// double-word, which is the same outcome the fielded bootloader had.
				this->currentPhase = Phase::Receiving;
			}
			const Error error = this->upload.write(command.offset, command.payload,
				command.payloadLength);
			this->recentError = error;
			if(failed(error)) {
				hw::logChar(marker(error));
			}
			else {
				hw::ledToggle(hw::Led::Frame);
				hw::logChar('O');
			}
			this->expiry = now + config::sessionSilence;
			break;
		}

		case CommandKind::Control:
			this->handleControl(command, now);
			break;

		default:
			break;
		}
	}

	//----------
	void
	Bootloader::replyError(const Command & command, const char * verb, Error error)
	{
		this->recentError = error;
		if(!command.replyAllowed) {
			return;
		}
		this->link.beginReply(command, verb, 2);
		this->link.fieldBool("ok", false);
		this->link.fieldUint("err", code(error));
		this->link.endReply(command);
	}

	//----------
	void
	Bootloader::handleControl(const Command & command, uint32_t now)
	{
		switch(command.verb) {
		case Verb::Status:
			this->replyStatus(command);
			break;

		case Verb::Begin: {
			if(!command.hasLength || !command.hasCrc || !command.hasChunk) {
				this->replyError(command, "begin", Error::BadParam);
				break;
			}
			const uint32_t base = command.hasBase ? command.base : config::appBase;
			const Error error = this->upload.declare(command.length, command.crc,
				command.chunk, base);
			if(failed(error)) {
				this->replyError(command, "begin", error);
				break;
			}
			this->upload.beginErase(base);
			this->currentPhase = Phase::Erasing;
			this->noDeadline = false;
			this->recentError = Error::None;
			// Answered when the last page is done, not now: the point of the verb is that the host
			// learns when the board is actually ready for data.
			this->beginPending = true;
			this->beginRequest = command;
			hw::logChar('E');
			break;
		}

		case Verb::Map: {
			if(!command.replyAllowed) {
				break;
			}
			const uint32_t chunk = command.hasChunk ? command.chunk : this->upload.chunkBytes();
			const uint32_t length = this->upload.declared()
				? this->upload.length()
				: this->upload.highWater();
			uint8_t bits[config::bitmapBytes];
			const size_t bytes = this->upload.bitmap().renderChunks(chunk, length,
				bits, sizeof(bits));
			if(bytes == 0 && length != 0) {
				this->replyError(command, "map", Error::BadParam);
				break;
			}
			// The bin8 header carries one length byte, so a bitmap over 255 bytes cannot be sent
			// in this shape. `begin` already refuses a chunk that small, but `map` accepts its
			// own chunk override, so the same wall is checked here -- and refused rather than
			// truncated. A truncated bitmap reads as "everything past here is missing", which
			// sends a host round the repair loop for ever.
			if(bytes > 255) {
				this->replyError(command, "map", Error::BadParam);
				break;
			}
			this->link.beginReply(command, "map", 3);
			this->link.fieldUint("chunk", chunk);
			this->link.fieldUint("len", length);
			this->link.fieldBinary("map", bits, (uint8_t) bytes);
			this->link.endReply(command);
			break;
		}

		case Verb::Verify: {
			if(!command.replyAllowed) {
				break;
			}
			const uint32_t length = this->upload.declared()
				? this->upload.length()
				: this->upload.highWater();
			const uint32_t computed = crcOverFlash(this->upload.base(), length);
			const bool ok = this->upload.declared() && computed == this->upload.crc32();
			this->link.beginReply(command, "verify", 3);
			this->link.fieldBool("ok", ok);
			this->link.fieldUint("crc", computed);
			this->link.fieldUint("len", length);
			this->link.endReply(command);
			break;
		}

		case Verb::Run:
			this->tryRun(&command);
			break;

		case Verb::Adopt: {
			if(!command.hasId || command.id <= 0) {
				this->replyError(command, "adopt", Error::BadParam);
				break;
			}
			this->adopt(command.id, IdSource::Adopt);
			if(command.replyAllowed) {
				this->link.beginReply(command, "adopt", 1);
				this->link.fieldInt("id", this->myId);
				this->link.endReply(command);
			}
			break;
		}

		case Verb::Reset:
			if(command.replyAllowed) {
				this->link.beginReply(command, "reset", 1);
				this->link.fieldBool("ok", true);
				this->link.endReply(command);
			}
			hw::txDrain();
			hw::reset();
			break;

		default:
			// Answered as `?`, not as the name of some verb we do understand. A host that matches
			// replies by verb -- which `fw_session` does -- would read a refusal labelled "status"
			// as a status reply, which is a worse answer than the silence this replaced.
			this->replyError(command, "?", Error::UnknownVerb);
			break;
		}

		(void) now;
	}

	//----------
	void
	Bootloader::replyStatus(const Command & command)
	{
		if(!command.replyAllowed) {
			return;
		}

		const portal_app_descriptor_t * descriptor = nullptr;
		uint32_t appBase = 0;
		// Report whatever is actually installed, from whichever bank holds it, so a host can see
		// that a board is still running a legacy-base application without having to infer it.
		if(!regionBlank(config::appBase, 0x2000) && vectorTableValid(config::appBase)) {
			descriptor = descriptorAt(config::appBase);
			appBase = config::appBase;
		}
		else if(vectorTableValid(config::appBaseLegacy)) {
			descriptor = descriptorAt(config::appBaseLegacy);
			appBase = config::appBaseLegacy;
		}

		const uint8_t fields = (uint8_t) (14 + (appBase != 0 ? 1 : 0));
		this->link.beginReply(command, "status", fields);
		this->link.fieldUint("v", config::protocolVersion);
		this->link.fieldInt("id", this->myId);
		this->link.fieldString("src", sourceName(this->source));
		this->link.fieldUint("s", this->identity.valid ? this->identity.serial : 0);
		{
			uint8_t bytes[12];
			memcpy(bytes, this->uid, sizeof(bytes));
			this->link.fieldBinary("uid", bytes, sizeof(bytes));
		}
		this->link.fieldUint("base", this->upload.base());
		this->link.fieldUint("cap", this->upload.capacity());
		this->link.fieldUint("chunk", config::chunkMax);
		this->link.fieldUint("st", (uint32_t) this->currentPhase);
		this->link.fieldUint("prog", this->upload.erasePages());
		this->link.fieldUint("wp", this->upload.highWater());
		this->link.fieldUint("n", this->upload.received());
		this->link.fieldUint("err", code(this->recentError));
		// Bit 0: a frame arrived longer than the window and was dropped. Bit 1: the UART receive
		// ring overran. Both were already latched and both were thrown away, which meant the two
		// explanations for "the upload keeps losing frames" -- frames too long for the window, or
		// frames arriving faster than this board can drain them -- were the two things a host
		// could not ask about.
		//
		// Accumulated rather than sampled, because `hw::ringOverran()` clears as it reads: asking
		// twice would otherwise report the overrun to whichever `status` happened to land first
		// and hide it from every one after.
		if(this->link.overflowed()) {
			this->dropFlags |= 1u;
		}
		if(hw::ringOverran()) {
			this->dropFlags |= 2u;
		}
		this->link.fieldUint("drops", this->dropFlags);
		if(appBase != 0) {
			this->link.fieldMap("app", 2);
			this->link.fieldUint("base", descriptor != nullptr ? descriptor->app_base : appBase);
			if(descriptor != nullptr) {
				this->link.fieldStringBounded("ver", descriptor->version,
					PORTAL_APP_VERSION_BYTES);
			}
			else {
				this->link.fieldString("ver", "");
			}
		}
		this->link.endReply(command);
	}

	//----------
	void
	Bootloader::tryRun(const Command * command)
	{
		const RunDecision decision = decideRun(
			this->upload.declared() ? this->upload.length() : 0,
			this->upload.declared() ? this->upload.crc32() : 0,
			this->upload.base());

		if(command != nullptr && command->replyAllowed) {
			this->link.beginReply(*command, "run", 3);
			this->link.fieldBool("ok", decision.ok);
			this->link.fieldUint("err", code(decision.error));
			this->link.fieldUint("base", decision.base);
			this->link.endReply(*command);
		}

		if(!decision.ok) {
			// Nothing to start. Stay resident for good rather than resetting into the same
			// decision a moment later: a board that cannot run is one a host needs to be able to
			// reach, and a boot loop looks exactly like a hardware fault from the outside.
			this->recentError = decision.error;
			this->currentPhase = Phase::Held;
			this->noDeadline = true;
			hw::logChar('x');
			return;
		}

		hw::logChar('J');
		hw::txDrain();
		hw::runApplication(decision.base);

		// Only reached in tests, where the terminal actions record and return.
		this->currentPhase = Phase::Held;
		this->noDeadline = true;
	}

} // namespace bl
