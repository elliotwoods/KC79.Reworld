// Turning bytes on the RS485 bus into one decoded command, and writing one reply back.
//
// # Parse, then verify, then act
//
// Every frame is decoded into a `Command` *before* anything happens as a result of it. That
// ordering is the whole reason the trailing CRC is worth having: the CRC arrives after the body,
// so a parser that acted as it read would have already erased a bank or jumped to an address by
// the time it discovered the frame was corrupt. Nothing here touches flash; the caller does, after
// `receive()` has returned a command it has already checked.
//
// # Exactly one frame is visible at a time
//
// [`FrameWindow`] below buffers one complete COBS frame out of the UART and presents *only* that
// frame to the codec. Two separate problems make this necessary rather than tidy.
//
// The msgpack library blocks when it runs out of bytes: `waitForData` spins for 100 ms, and
// `NotArduino`'s `readBytes` spins *forever*. Both are reasonable for a stream that will
// eventually deliver, and neither is acceptable in a loop that also has to service a watchdog.
// A window that only ever holds a whole frame means the parser can never ask for a byte that has
// not already arrived.
//
// The second is subtler and was found by test rather than by reading. `COBSRWStream::available()`
// calls `decodeIncoming()`, and `decodeIncoming` only stops at a packet boundary once the reader
// has consumed a byte of that packet -- so calling `available()` *twice* before the first `read()`
// decodes straight through the delimiter and appends the following packet to the current one. The
// merged tail is then discarded when the reader advances, so the second frame vanishes without a
// trace. Every `waitForData` inside every parse calls `available()`, so this is reachable from
// ordinary parsing, and two frames arriving back to back is the normal case during an upload
// rather than an unlucky one. With nothing behind the delimiter to decode, it cannot happen.
#pragma once

#include <stdint.h>
#include <stddef.h>

#include "msgpack/COBSRWStream.hpp"

#include "bl/config.hpp"
#include "bl/errors.hpp"

namespace bl {

	/// One COBS frame, read from a byte stream and presented in isolation.
	///
	/// Reads come from the buffered frame; writes pass straight through to the underlying stream,
	/// so one `COBSRWStream` can still both parse a request and compose a reply.
	class FrameWindow : public msgpack::Stream {
	public:
		explicit FrameWindow(msgpack::Stream & sink) : sink(sink) {}

		/// Pull bytes from `source` until a whole frame is held. Returns true when one is ready.
		///
		/// Partial frames accumulate across calls, so a frame split across two UART interrupts is
		/// assembled rather than dropped. A frame longer than the buffer is discarded at its
		/// delimiter and the next one is looked for -- dropping the offender rather than the rest
		/// of the stream with it.
		bool load(msgpack::Stream & source);

		/// Release the current frame and make room for the next.
		void release();

		bool holding() const { return this->ready; }
		/// Whether the last released frame was dropped for being too long.
		bool overflowed() const { return this->sawOverflow; }

		int available() override;
		int read() override;
		int peek() override;
		size_t write(uint8_t value) override { return this->sink.write(value); }
		size_t write(const uint8_t * buffer, size_t size) override
		{
			return this->sink.write(buffer, size);
		}
		void flush() override { this->sink.flush(); }

	private:
		msgpack::Stream & sink;
		/// Sized for the largest legitimate frame: a 256-byte payload, its checksum, the map and
		/// envelope around it, the trailer, and COBS's own overhead.
		uint8_t frame[config::frameBufferBytes];
		size_t length = 0;
		size_t position = 0;
		bool ready = false;
		bool overflow = false;
		bool sawOverflow = false;
	};

	/// What a frame turned out to be.
	enum class CommandKind : uint8_t {
		/// Nothing to do: no complete frame, not addressed to us, or dropped.
		None,
		/// A legacy `"FW"`-prefixed announce. Extends residency and nothing else.
		Announce,
		/// Legacy `"ER"`: erase the bank.
		Erase,
		/// Legacy `"RU"`: start the application.
		RunLegacy,
		/// A data payload at an offset.
		Data,
		/// A `bl` control-plane verb.
		Control,
		/// A frame that parsed far enough to know it was for us, but was then rejected.
		Rejected,
	};

	enum class Verb : uint8_t {
		None,
		Status,
		Begin,
		Map,
		Verify,
		Run,
		Adopt,
		Reset,
	};

	struct Command {
		CommandKind kind = CommandKind::None;
		Error error = Error::None;

		/// Whether this board may answer. True for a unicast to our own id, and for a broadcast
		/// carrying a selector that matched us. Never true for an unselected broadcast: six boards
		/// answering one frame on a half-duplex bus is a collision, not a conversation.
		bool replyAllowed = false;
		/// The sequence number to echo, from the request's trailer.
		uint8_t seq = 0;

		// Data frames
		uint32_t offset = 0;
		const uint8_t * payload = nullptr;
		uint32_t payloadLength = 0;

		// Control frames
		Verb verb = Verb::None;
		bool hasLength = false;
		bool hasCrc = false;
		bool hasChunk = false;
		bool hasBase = false;
		bool hasId = false;
		uint32_t length = 0;
		uint32_t crc = 0;
		uint32_t chunk = 0;
		uint32_t base = 0;
		int8_t id = 0;
	};

	/// Frame parser and reply writer over one COBS/msgpack stream.
	class Link {
	public:
		explicit Link(msgpack::Stream & io);

		/// Who we are, for addressing and selector matching.
		void setAddress(int8_t id);
		void setIdentity(uint32_t serial, const uint32_t uid[3]);
		int8_t address() const { return this->myId; }

		/// Whether at least one complete frame is waiting.
		bool pending();

		/// Consume one frame. Returns a command whose `kind` is `None` when there was nothing for
		/// us. Never blocks.
		Command receive();

		// ---- Reply construction ------------------------------------------------------------
		//
		// The map size has to be written before its contents, so the caller counts its own
		// fields. Awkward, and much smaller than the alternative of buffering a reply to measure
		// it -- which in a 16 kB image is the kind of convenience that costs a feature elsewhere.

		/// Whether a frame has been dropped for being longer than the window can hold.
		///
		/// Sticky once set; the window latches it and nothing clears it. Surfaced as bit 0 of
		/// `status.drops`, which is the only way a host can tell "your frames are too big" from
		/// the several other reasons a chunk might not have landed.
		bool overflowed() const { return this->window.overflowed(); }

		void beginReply(const Command & request, const char * verb, uint8_t fieldCount);
		void fieldUint(const char * name, uint32_t value);
		void fieldInt(const char * name, int8_t value);
		void fieldBool(const char * name, bool value);
		void fieldString(const char * name, const char * value);
		/// A string that may not be NUL-terminated, such as the descriptor's fixed-width version
		/// field. Stops at `maxSize` or the first NUL, whichever comes first.
		void fieldStringBounded(const char * name, const char * value, uint8_t maxSize);
		void fieldBinary(const char * name, const uint8_t * value, uint8_t size);
		/// Open a nested map of `fieldCount` entries. The caller writes them next.
		void fieldMap(const char * name, uint8_t fieldCount);
		void endReply(const Command & request);

	private:
		bool parseBody(Command & command, size_t arraySize);
		bool parseMagic(Command & command);
		bool parseMap(Command & command);
		bool parseControl(Command & command);
		bool parseData(Command & command, uint32_t offset);
		bool skipValue();
		bool checkTrailer(Command & command, size_t arraySize);

		msgpack::Stream & io;
		FrameWindow window;
		msgpack::COBSRWStream cobs;
		int8_t myId = 0;
		uint32_t serial = 0;
		uint32_t uid[3] = {0, 0, 0};

		/// One static receive buffer for the largest legitimate payload.
		///
		/// Not a VLA sized from the wire, which is what the fielded bootloader did: a frame
		/// claiming 60,000 bytes would smash the stack of an image that can only be recovered with
		/// a debug probe. The size is checked against this bound before a byte is read.
		uint8_t payloadBuffer[config::payloadBufferBytes];
	};

} // namespace bl
