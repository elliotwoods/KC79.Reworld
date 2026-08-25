#include "bl/link.hpp"

#include "msgpack/deserialize.hpp"
#include "msgpack/serialize.hpp"

#include <string.h>

namespace bl {
	namespace {
		constexpr int32_t broadcastAddress = -1;
		constexpr int32_t hostAddress = 0;

		/// `0xC4 size <bytes>`.
		///
		/// Hand-written because `serialize.hpp` declares `writeBinary8(Print&, const char*, ...)`
		/// while `serialize.cpp` defines `writeBinary8(Print&, const uint8_t*, ...)`. They are
		/// different overloads, so the declared one has never had a definition and calling it
		/// fails at link. Three bytes of our own is cheaper than a submodule commit.
		void writeBin8(msgpack::Print & stream, const uint8_t * value, uint8_t size) {
			msgpack::writeRawByte(stream, 0xC4);
			msgpack::writeRawByte(stream, size);
			msgpack::writeRaw(stream, value, size);
		}

		void writeKey(msgpack::Print & stream, const char * name) {
			msgpack::writeString5(stream, name, (uint8_t) strlen(name));
		}

		/// XOR of little-endian 16-bit words: the payload checksum every data frame carries.
		///
		/// Byte-identical in behaviour to `Utils::calcCheckSum` in the C++ Router and to
		/// `checksum_xor16` in `router-proto`, but written over bytes rather than by casting the
		/// buffer to `uint16_t*` -- the original does the cast, which is an alignment fault
		/// waiting to happen on a payload that did not start on an even address.
		uint16_t checksumXor16(const uint8_t * data, uint32_t length) {
			uint16_t value = 0;
			uint32_t index = 0;
			while(index + 1 < length) {
				value ^= (uint16_t) ((uint16_t) data[index] | ((uint16_t) data[index + 1] << 8));
				index += 2;
			}
			if(index < length) {
				value ^= (uint16_t) data[index];
			}
			return value;
		}

		Verb verbFromName(const char * name, uint8_t size) {
			switch(size) {
			case 3:
				if(memcmp(name, "map", 3) == 0) return Verb::Map;
				if(memcmp(name, "run", 3) == 0) return Verb::Run;
				break;
			case 5:
				if(memcmp(name, "begin", 5) == 0) return Verb::Begin;
				if(memcmp(name, "adopt", 5) == 0) return Verb::Adopt;
				if(memcmp(name, "reset", 5) == 0) return Verb::Reset;
				break;
			case 6:
				if(memcmp(name, "status", 6) == 0) return Verb::Status;
				if(memcmp(name, "verify", 6) == 0) return Verb::Verify;
				break;
			default:
				break;
			}
			return Verb::None;
		}
	}

	// ---- FrameWindow -------------------------------------------------------------------------

	//----------
	bool
	FrameWindow::load(msgpack::Stream & source)
	{
		if(this->ready) {
			return true;
		}

		while(source.available() > 0) {
			const int value = source.read();
			if(value < 0) {
				break;
			}

			if(this->length < sizeof(this->frame)) {
				this->frame[this->length++] = (uint8_t) value;
			}
			else {
				// Longer than any legitimate frame. Keep consuming to the delimiter so the *next*
				// frame is still found -- abandoning the stream here would turn one oversized
				// frame into a permanently desynchronised link.
				this->overflow = true;
			}

			if(value == 0x00) {
				if(this->overflow) {
					this->sawOverflow = true;
					this->length = 0;
					this->overflow = false;
					continue;
				}
				this->ready = true;
				this->position = 0;
				return true;
			}
		}
		return false;
	}

	//----------
	void
	FrameWindow::release()
	{
		this->length = 0;
		this->position = 0;
		this->ready = false;
		this->overflow = false;
		this->sawOverflow = false;
	}

	//----------
	int
	FrameWindow::available()
	{
		if(!this->ready) {
			return 0;
		}
		return (int) (this->length - this->position);
	}

	//----------
	int
	FrameWindow::read()
	{
		if(this->available() <= 0) {
			return -1;
		}
		return (int) this->frame[this->position++];
	}

	//----------
	int
	FrameWindow::peek()
	{
		if(this->available() <= 0) {
			return -1;
		}
		return (int) this->frame[this->position];
	}

	// ---- Link ---------------------------------------------------------------------------------

	//----------
	Link::Link(msgpack::Stream & io)
	: io(io)
	, window(io)
	, cobs(window)
	{
	}

	//----------
	void
	Link::setAddress(int8_t id)
	{
		this->myId = id;
	}

	//----------
	void
	Link::setIdentity(uint32_t serialNumber, const uint32_t ownUid[3])
	{
		this->serial = serialNumber;
		this->uid[0] = ownUid[0];
		this->uid[1] = ownUid[1];
		this->uid[2] = ownUid[2];
	}

	//----------
	bool
	Link::pending()
	{
		// Deliberately does not touch the codec: `COBSRWStream::available()` has side effects on
		// packet boundaries, and a caller polling for work must not be able to change what the
		// next parse sees.
		return this->window.load(this->io);
	}

	//----------
	Command
	Link::receive()
	{
		Command command;

		if(!this->window.load(this->io)) {
			return command;
		}

		// Every path out of here releases the window and resets the codec, so one call consumes
		// exactly one frame whatever happens to it.
		struct Guard {
			FrameWindow & window;
			msgpack::COBSRWStream & cobs;
			~Guard()
			{
				this->cobs.nextIncomingPacket();
				this->window.release();
			}
		} guard{this->window, this->cobs};

		if(!this->cobs.isStartOfIncomingPacket()) {
			this->cobs.nextIncomingPacket();
		}
		if(this->cobs.available() <= 0) {
			return command;
		}

		size_t arraySize = 0;
		if(!msgpack::readArraySize(this->cobs, arraySize) || arraySize < 3) {
			return command;
		}

		int32_t target = 0;
		int32_t source = 0;
		if(!msgpack::readInt<int32_t>(this->cobs, target)
			|| !msgpack::readInt<int32_t>(this->cobs, source)) {
			return command;
		}

		const bool unicastToUs = (this->myId > 0) && (target == (int32_t) this->myId);
		const bool broadcast = (target == broadcastAddress);
		if(!unicastToUs && !broadcast) {
			// Somebody else's traffic, including our own echo if the transceiver hears itself
			// (which arrives as target 0, the host's address). Silence is the only correct
			// response: a unicast poll addressed to a board running its application must not draw
			// a reply from a board sitting in its bootloader.
			return command;
		}

		command.replyAllowed = unicastToUs;

		if(!this->parseBody(command, arraySize)) {
			if(command.error != Error::None) {
				command.kind = CommandKind::Rejected;
				return command;
			}
			command.kind = CommandKind::None;
			return command;
		}

		if(!this->checkTrailer(command, arraySize)) {
			command.kind = CommandKind::Rejected;
			command.error = Error::Crc16;
			command.replyAllowed = false;
			return command;
		}

		return command;
	}

	//----------
	bool
	Link::parseBody(Command & command, size_t arraySize)
	{
		(void) arraySize;

		msgpack::DataType type;
		if(!msgpack::getNextDataType(this->cobs, type)) {
			return false;
		}

		if(type == msgpack::DataType::String5) {
			return this->parseMagic(command);
		}
		if(type == msgpack::DataType::Map) {
			return this->parseMap(command);
		}
		// A ping (nil), or anything else. The bootloader has nothing to say about it.
		return false;
	}

	//----------
	bool
	Link::parseMagic(Command & command)
	{
		char word[config::wordBufferBytes];
		uint8_t size = 0;
		if(!msgpack::readString5(this->cobs, word, (uint8_t) sizeof(word), size)) {
			return false;
		}

		// Any word beginning "FW" is an announce.
		//
		// The fielded bootloader parsed this into a 3-byte buffer, so the 7-byte "FW!KC79" the
		// application listens for was a *format error* to it -- which is why the host has to
		// interleave both words rather than simply sending the long one. Accepting the prefix
		// costs nothing and means a future host can stop caring.
		if(size >= 2 && word[0] == 'F' && word[1] == 'W') {
			command.kind = CommandKind::Announce;
			return true;
		}
		if(size == 2 && word[0] == 'E' && word[1] == 'R') {
			command.kind = CommandKind::Erase;
			return true;
		}
		if(size == 2 && word[0] == 'R' && word[1] == 'U') {
			command.kind = CommandKind::RunLegacy;
			return true;
		}
		return false;
	}

	//----------
	bool
	Link::parseMap(Command & command)
	{
		size_t entries = 0;
		if(!msgpack::readMapSize(this->cobs, entries) || entries < 1) {
			return false;
		}

		msgpack::DataType keyType;
		if(!msgpack::getNextDataType(this->cobs, keyType)) {
			return false;
		}

		// An integer key is a firmware data frame: `{offset: bin(checksum ++ data)}`. A string key
		// is a command. Nothing else has ever been sent to a bootloader.
		if(msgpack::isInt(keyType)) {
			int32_t offset = 0;
			if(!msgpack::readInt<int32_t>(this->cobs, offset) || offset < 0) {
				return false;
			}
			return this->parseData(command, (uint32_t) offset);
		}

		if(keyType != msgpack::DataType::String5 && keyType != msgpack::DataType::String8) {
			return false;
		}

		char name[config::wordBufferBytes];
		uint8_t size = 0;
		if(!msgpack::readString5(this->cobs, name, (uint8_t) sizeof(name), size)) {
			return false;
		}
		if(size == 2 && memcmp(name, "bl", 2) == 0) {
			return this->parseControl(command);
		}
		return false;
	}

	//----------
	bool
	Link::parseData(Command & command, uint32_t offset)
	{
		uint16_t declared = 0;
		if(!msgpack::readBinarySize(this->cobs, declared)) {
			return false;
		}

		// Bounds first, before a single byte is read and before any arithmetic that could wrap.
		// A payload shorter than its own checksum would underflow the subtraction below.
		if(declared < config::payloadChecksumBytes || declared > config::payloadBufferBytes) {
			command.error = Error::Format;
			return false;
		}

		// Read in slices bounded by what has actually arrived. `readBytes` on the non-Arduino
		// path loops until it has everything, with no timeout, so handing it a length larger than
		// the buffer holds would hang the bootloader until the watchdog fired.
		uint32_t taken = 0;
		while(taken < declared) {
			const int ready = this->cobs.available();
			if(ready <= 0) {
				command.error = Error::Format;
				return false;
			}
			uint32_t take = (uint32_t) ready;
			if(take > declared - taken) {
				take = declared - taken;
			}
			this->cobs.readBytes((char *) this->payloadBuffer + taken, take);
			taken += take;
		}

		const uint32_t bodyLength = declared - config::payloadChecksumBytes;
		const uint8_t * body = this->payloadBuffer + config::payloadChecksumBytes;
		const uint16_t transmitted = (uint16_t) ((uint16_t) this->payloadBuffer[0]
			| ((uint16_t) this->payloadBuffer[1] << 8));
		if(checksumXor16(body, bodyLength) != transmitted) {
			command.error = Error::Xor;
			return false;
		}

		command.kind = CommandKind::Data;
		command.offset = offset;
		command.payload = body;
		command.payloadLength = bodyLength;
		return true;
	}

	//----------
	bool
	Link::parseControl(Command & command)
	{
		size_t entries = 0;
		if(!msgpack::readMapSize(this->cobs, entries)) {
			return false;
		}

		bool selectorSeen = false;
		bool selectorMatched = false;

		for(size_t entry = 0; entry < entries; entry++) {
			msgpack::DataType keyType;
			if(!msgpack::getNextDataType(this->cobs, keyType)) {
				return false;
			}
			if(keyType != msgpack::DataType::String5 && keyType != msgpack::DataType::String8) {
				return false;
			}

			char name[config::wordBufferBytes];
			uint8_t size = 0;
			if(!msgpack::readString5(this->cobs, name, (uint8_t) sizeof(name), size)) {
				return false;
			}

			if(size == 1 && name[0] == 'q') {
				char verb[config::wordBufferBytes];
				uint8_t verbSize = 0;
				if(!msgpack::readString5(this->cobs, verb, (uint8_t) sizeof(verb), verbSize)) {
					return false;
				}
				command.verb = verbFromName(verb, verbSize);
			}
			else if(size == 1 && name[0] == 's') {
				int32_t value = 0;
				if(!msgpack::readInt<int32_t>(this->cobs, value)) {
					return false;
				}
				selectorSeen = true;
				// A board with no identity has serial 0, and 0 is never a valid serial, so it
				// cannot be selected by one -- only by UID. That is deliberate: a rack of
				// unprovisioned boards would otherwise all answer the same selector.
				if(this->serial != 0 && (uint32_t) value == this->serial) {
					selectorMatched = true;
				}
			}
			else if(size == 3 && memcmp(name, "uid", 3) == 0) {
				uint16_t declared = 0;
				if(!msgpack::readBinarySize(this->cobs, declared)) {
					return false;
				}
				if(declared != 12) {
					return false;
				}
				uint8_t bytes[12];
				uint32_t taken = 0;
				while(taken < declared) {
					const int ready = this->cobs.available();
					if(ready <= 0) {
						return false;
					}
					uint32_t take = (uint32_t) ready;
					if(take > declared - taken) {
						take = declared - taken;
					}
					this->cobs.readBytes((char *) bytes + taken, take);
					taken += take;
				}
				selectorSeen = true;
				uint8_t ours[12];
				memcpy(ours, this->uid, sizeof(ours));
				if(memcmp(bytes, ours, sizeof(ours)) == 0) {
					selectorMatched = true;
				}
			}
			else if(size == 3 && memcmp(name, "len", 3) == 0) {
				int32_t value = 0;
				if(!msgpack::readInt<int32_t>(this->cobs, value)) {
					return false;
				}
				command.hasLength = true;
				command.length = (uint32_t) value;
			}
			else if(size == 3 && memcmp(name, "crc", 3) == 0) {
				int32_t value = 0;
				if(!msgpack::readInt<int32_t>(this->cobs, value)) {
					return false;
				}
				command.hasCrc = true;
				command.crc = (uint32_t) value;
			}
			else if(size == 5 && memcmp(name, "chunk", 5) == 0) {
				int32_t value = 0;
				if(!msgpack::readInt<int32_t>(this->cobs, value)) {
					return false;
				}
				command.hasChunk = true;
				command.chunk = (uint32_t) value;
			}
			else if(size == 4 && memcmp(name, "base", 4) == 0) {
				int32_t value = 0;
				if(!msgpack::readInt<int32_t>(this->cobs, value)) {
					return false;
				}
				command.hasBase = true;
				command.base = (uint32_t) value;
			}
			else if(size == 2 && memcmp(name, "id", 2) == 0) {
				int32_t value = 0;
				if(!msgpack::readInt<int32_t>(this->cobs, value)) {
					return false;
				}
				command.hasId = true;
				command.id = (int8_t) value;
			}
			else if(!this->skipValue()) {
				// An unrecognised key is skipped rather than rejected, so a host built against a
				// later revision of this protocol can still talk to this bootloader.
				return false;
			}
		}

		if(command.verb == Verb::None) {
			command.error = Error::UnknownVerb;
			return false;
		}

		// Addressing, finally, now that any selector has been seen.
		if(selectorSeen) {
			if(!selectorMatched) {
				// Named somebody else. Not ours to act on *or* answer -- acting would erase the
				// wrong board's flash.
				return false;
			}
			command.replyAllowed = true;
		}
		else if(!command.replyAllowed) {
			// An unselected broadcast. Acted on, never answered.
			if(command.verb == Verb::Adopt) {
				// Except this one: every board adopting the same id from one frame is how a bus
				// becomes unusable.
				return false;
			}
		}

		command.kind = CommandKind::Control;
		return true;
	}

	//----------
	bool
	Link::skipValue()
	{
		msgpack::DataType type;
		if(!msgpack::getNextDataType(this->cobs, type)) {
			return false;
		}

		switch(type) {
		case msgpack::DataType::Nil:
			return msgpack::readNil(this->cobs);
		case msgpack::DataType::Bool: {
			bool value = false;
			return msgpack::readBool(this->cobs, value);
		}
		case msgpack::DataType::String5:
		case msgpack::DataType::String8: {
			char scratch[config::wordBufferBytes];
			uint8_t size = 0;
			return msgpack::readString5(this->cobs, scratch, (uint8_t) sizeof(scratch), size);
		}
		case msgpack::DataType::Binary8:
		case msgpack::DataType::Binary16: {
			uint16_t declared = 0;
			if(!msgpack::readBinarySize(this->cobs, declared)) {
				return false;
			}
			uint32_t taken = 0;
			while(taken < declared) {
				const int ready = this->cobs.available();
				if(ready <= 0) {
					return false;
				}
				uint32_t take = (uint32_t) ready;
				if(take > declared - taken) {
					take = declared - taken;
				}
				char scratch[32];
				if(take > sizeof(scratch)) {
					take = sizeof(scratch);
				}
				this->cobs.readBytes(scratch, take);
				taken += take;
			}
			return true;
		}
		default:
			break;
		}

		if(msgpack::isInt(type)) {
			int32_t value = 0;
			return msgpack::readInt<int32_t>(this->cobs, value);
		}
		// Nested maps and arrays are not part of this protocol; refusing them keeps the skipper
		// from needing recursion, which is not something a 2 kB stack should be doing on
		// attacker-shaped input.
		return false;
	}

	//----------
	bool
	Link::checkTrailer(Command & command, size_t arraySize)
	{
		if(arraySize < 5) {
			// A legacy 3-element frame, which the fielded Router sends and nothing else. Refusing
			// it outright would break every host that has not been updated -- which, the first
			// time this firmware runs, is all of them.
			//
			// But accepting it on the strength of the element count alone opens a downgrade: one
			// flipped bit turns the `0x95` header of a *trailered* frame into `0x93`, and the
			// frame is then accepted with its integrity check skipped and its trailer bytes left
			// lying in the packet. So a genuine legacy frame has to prove it is one, by ending
			// exactly where it said it would. A downgraded frame has five bytes left over and is
			// rejected; `0x94` has no legitimate meaning at all.
			if(arraySize != 3) {
				return false;
			}
			return this->cobs.available() == 0;
		}

		int32_t seq = 0;
		if(!msgpack::readInt<int32_t>(this->cobs, seq)) {
			return false;
		}

		// Snapshot between the two reads. The running CRC covers every byte the parser has
		// consumed, so at this instant it holds exactly what the sender's CRC covered -- and one
		// read later it would have folded the CRC field into its own check.
		const uint16_t computed = this->cobs.getRxRunningCRC();

		int32_t transmitted = 0;
		if(!msgpack::readInt<int32_t>(this->cobs, transmitted)) {
			return false;
		}

		if((uint16_t) transmitted != computed) {
			return false;
		}

		command.seq = (uint8_t) seq;
		return true;
	}

	// ---- Replies ---------------------------------------------------------------------------

	//----------
	void
	Link::beginReply(const Command & request, const char * verb, uint8_t fieldCount)
	{
		(void) request;
		msgpack::writeArraySize4(this->cobs, 5);
		msgpack::writeIntU7(this->cobs, (uint8_t) hostAddress);
		msgpack::writeIntU7(this->cobs, (uint8_t) (this->myId < 0 ? 0 : this->myId));
		msgpack::writeMapSize4(this->cobs, 1);
		writeKey(this->cobs, "bl");
		msgpack::writeMapSize4(this->cobs, (uint8_t) (fieldCount + 1));
		writeKey(this->cobs, "q");
		msgpack::writeString5(this->cobs, verb, (uint8_t) strlen(verb));
	}

	//----------
	void
	Link::fieldUint(const char * name, uint32_t value)
	{
		writeKey(this->cobs, name);
		msgpack::writeIntU32(this->cobs, value);
	}

	//----------
	void
	Link::fieldInt(const char * name, int8_t value)
	{
		writeKey(this->cobs, name);
		msgpack::writeInt8(this->cobs, value);
	}

	//----------
	void
	Link::fieldBool(const char * name, bool value)
	{
		writeKey(this->cobs, name);
		msgpack::writeBool(this->cobs, value);
	}

	//----------
	void
	Link::fieldString(const char * name, const char * value)
	{
		writeKey(this->cobs, name);
		msgpack::writeString5(this->cobs, value, (uint8_t) strlen(value));
	}

	//----------
	void
	Link::fieldStringBounded(const char * name, const char * value, uint8_t maxSize)
	{
		uint8_t size = 0;
		while(size < maxSize && value[size] != '\0') {
			size++;
		}
		writeKey(this->cobs, name);
		// String5 tops out at 31 bytes; the descriptor's version field is 40. `writeString8`
		// carries an explicit length byte and both are read by `readString` on the host.
		if(size < 32) {
			msgpack::writeString5(this->cobs, value, size);
		}
		else {
			msgpack::writeString8(this->cobs, value, size);
		}
	}

	//----------
	void
	Link::fieldBinary(const char * name, const uint8_t * value, uint8_t size)
	{
		writeKey(this->cobs, name);
		writeBin8(this->cobs, value, size);
	}

	//----------
	void
	Link::fieldMap(const char * name, uint8_t fieldCount)
	{
		writeKey(this->cobs, name);
		msgpack::writeMapSize4(this->cobs, fieldCount);
	}

	//----------
	void
	Link::endReply(const Command & request)
	{
		// Forced widths, never minimised, so a receiver knows the trailer is always the last five
		// bytes without re-parsing the body to find where it starts.
		msgpack::writeIntU8(this->cobs, request.seq);
		msgpack::writeIntU16(this->cobs, this->cobs.getTxRunningCRC());
		this->cobs.flush();
	}

} // namespace bl
