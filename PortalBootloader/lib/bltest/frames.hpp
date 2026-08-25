// Building host frames and reading replies, through the *real* codec.
//
// Every frame a test feeds the bootloader is COBS-encoded by `msgpack::COBSRWStream` and every
// reply is decoded by the same class the firmware uses. Nothing here re-implements the wire
// format, because a test suite that agreed with its own re-implementation of the protocol would
// prove nothing about the one that ships.
#pragma once

#include <stdint.h>
#include <stddef.h>
#include <string.h>

#include "msgpack/COBSRWStream.hpp"
#include "msgpack/deserialize.hpp"
#include "msgpack/serialize.hpp"

namespace bltest {

	/// A stream with two independent byte queues: what the host has sent, and what the bootloader
	/// has replied.
	///
	/// Deliberately a *plain* byte queue, with no notion of frames -- exactly like the target's
	/// UART ring, and including the awkward part of that: bytes from several frames sit in it at
	/// once, and a partially-arrived frame is indistinguishable from a complete one. Framing is
	/// `bl::FrameWindow`'s job, and giving the tests a stream that framed for it would be testing
	/// the wrong thing.
	class DuplexStream : public msgpack::Stream {
	public:
		// ---- The host side -------------------------------------------------------------------

		/// Queue a complete COBS frame (delimiter included) for the bootloader to read.
		void deliver(const uint8_t * frame, size_t length)
		{
			for(size_t index = 0; index < length; index++) {
				if(this->inboxLength < sizeof(this->inbox)) {
					this->inbox[this->inboxLength++] = frame[index];
				}
			}
		}

		/// Everything the bootloader has written, and how much of it there is.
		const uint8_t * sent() const { return this->outbox; }
		size_t sentLength() const { return this->outboxLength; }
		void clearSent() { this->outboxLength = 0; }
		/// How many complete frames the bootloader has replied with.
		size_t replyCount() const
		{
			size_t count = 0;
			for(size_t index = 0; index < this->outboxLength; index++) {
				if(this->outbox[index] == 0x00) {
					count++;
				}
			}
			return count;
		}

		/// How many complete frames are still queued for the bootloader to read.
		size_t queuedFrames() const
		{
			size_t count = 0;
			for(size_t index = this->inboxRead; index < this->inboxLength; index++) {
				if(this->inbox[index] == 0x00) {
					count++;
				}
			}
			return count;
		}

		// ---- msgpack::Stream ----------------------------------------------------------------

		int available() override
		{
			return (int) (this->inboxLength - this->inboxRead);
		}

		int read() override
		{
			if(this->available() <= 0) {
				return -1;
			}
			const uint8_t value = this->inbox[this->inboxRead++];
			this->compact();
			return (int) value;
		}

		int peek() override
		{
			if(this->available() <= 0) {
				return -1;
			}
			return (int) this->inbox[this->inboxRead];
		}

		size_t write(uint8_t value) override
		{
			if(this->outboxLength < sizeof(this->outbox)) {
				this->outbox[this->outboxLength++] = value;
				return 1;
			}
			return 0;
		}

		size_t write(const uint8_t * buffer, size_t size) override
		{
			size_t written = 0;
			for(size_t index = 0; index < size; index++) {
				written += this->write(buffer[index]);
			}
			return written;
		}

		void flush() override {}

	private:
		void compact()
		{
			if(this->inboxRead == this->inboxLength) {
				this->inboxRead = 0;
				this->inboxLength = 0;
			}
		}

		uint8_t inbox[8192] = {};
		size_t inboxLength = 0;
		size_t inboxRead = 0;

		uint8_t outbox[8192] = {};
		size_t outboxLength = 0;
	};

	/// A sink that COBS-encodes into a buffer, so tests can build a frame with the real encoder.
	class FrameSink : public msgpack::Stream {
	public:
		int available() override { return 0; }
		int read() override { return -1; }
		int peek() override { return -1; }
		size_t write(uint8_t value) override
		{
			if(this->length < sizeof(this->buffer)) {
				this->buffer[this->length++] = value;
				return 1;
			}
			return 0;
		}
		size_t write(const uint8_t * data, size_t size) override
		{
			size_t written = 0;
			for(size_t index = 0; index < size; index++) {
				written += this->write(data[index]);
			}
			return written;
		}
		void flush() override {}

		const uint8_t * bytes() const { return this->buffer; }
		size_t size() const { return this->length; }
		void clear() { this->length = 0; }

	private:
		uint8_t buffer[8192] = {};
		size_t length = 0;
	};

	/// Builds one frame through a real `COBSRWStream`, so the encoding under test is the shipping
	/// one. `seq >= 0` appends a `[seq, crc16]` trailer computed by the same running CRC the
	/// firmware verifies against.
	class FrameBuilder {
	public:
		FrameBuilder(int32_t target, int32_t source, int trailerSeq)
		: cobs(sink)
		, withTrailer(trailerSeq >= 0)
		, seq((uint8_t) (trailerSeq < 0 ? 0 : trailerSeq))
		{
			msgpack::writeArraySize4(this->cobs, this->withTrailer ? 5 : 3);
			msgpack::writeInt8(this->cobs, (int8_t) target);
			msgpack::writeInt8(this->cobs, (int8_t) source);
			(void) source;
		}

		msgpack::COBSRWStream & body() { return this->cobs; }

		/// Finish the frame and hand back the COBS bytes, delimiter included.
		void finish()
		{
			if(this->withTrailer) {
				msgpack::writeIntU8(this->cobs, this->seq);
				msgpack::writeIntU16(this->cobs, this->cobs.getTxRunningCRC());
			}
			this->cobs.flush();
		}

		const uint8_t * bytes() const { return this->sink.bytes(); }
		size_t size() const { return this->sink.size(); }

		/// Corrupt one byte of the encoded frame, to exercise the CRC gate. The delimiter and the
		/// leading COBS code byte are left alone so the frame still *frames*; what is being tested
		/// is the integrity check, not the framer.
		void corrupt(size_t index)
		{
			const_cast<uint8_t *>(this->sink.bytes())[index] ^= 0x01;
		}

	private:
		FrameSink sink;
		msgpack::COBSRWStream cobs;
		bool withTrailer;
		uint8_t seq;
	};

	// ---- Frame shorthands ----------------------------------------------------------------------

	/// `[target, 0, "WORD"]` -- an announce, erase or run.
	inline void magicFrame(FrameBuilder & builder, const char * word)
	{
		msgpack::writeString5(builder.body(), word, (uint8_t) strlen(word));
		builder.finish();
	}

	/// `[target, 0, {offset: bin(xor16 ++ data)}]` -- a firmware data frame.
	inline void dataFrame(FrameBuilder & builder, uint32_t offset, const uint8_t * data,
		uint32_t length, bool corruptChecksum = false)
	{
		uint16_t checksum = 0;
		{
			uint32_t index = 0;
			while(index + 1 < length) {
				checksum ^= (uint16_t) ((uint16_t) data[index] | ((uint16_t) data[index + 1] << 8));
				index += 2;
			}
			if(index < length) {
				checksum ^= (uint16_t) data[index];
			}
		}
		if(corruptChecksum) {
			checksum ^= 0xFFFF;
		}

		msgpack::writeMapSize4(builder.body(), 1);
		msgpack::writeIntU32(builder.body(), offset);
		msgpack::writeRawByte(builder.body(), 0xC4);
		msgpack::writeRawByte(builder.body(), (uint8_t) (length + 2));
		msgpack::writeRawByte(builder.body(), (uint8_t) (checksum & 0xFF));
		msgpack::writeRawByte(builder.body(), (uint8_t) (checksum >> 8));
		msgpack::writeRaw(builder.body(), data, length);
		builder.finish();
	}

	/// One `{"bl": {...}}` field, written by the caller between `controlBegin` and `finish`.
	inline void controlBegin(FrameBuilder & builder, const char * verb, uint8_t extraFields)
	{
		msgpack::writeMapSize4(builder.body(), 1);
		msgpack::writeString5(builder.body(), "bl", 2);
		msgpack::writeMapSize4(builder.body(), (uint8_t) (extraFields + 1));
		msgpack::writeString5(builder.body(), "q", 1);
		msgpack::writeString5(builder.body(), verb, (uint8_t) strlen(verb));
	}

	inline void controlUint(FrameBuilder & builder, const char * name, uint32_t value)
	{
		msgpack::writeString5(builder.body(), name, (uint8_t) strlen(name));
		msgpack::writeIntU32(builder.body(), value);
	}

	inline void controlInt(FrameBuilder & builder, const char * name, int8_t value)
	{
		msgpack::writeString5(builder.body(), name, (uint8_t) strlen(name));
		msgpack::writeInt8(builder.body(), value);
	}

	inline void controlBinary(FrameBuilder & builder, const char * name, const uint8_t * value,
		uint8_t size)
	{
		msgpack::writeString5(builder.body(), name, (uint8_t) strlen(name));
		msgpack::writeRawByte(builder.body(), 0xC4);
		msgpack::writeRawByte(builder.body(), size);
		msgpack::writeRaw(builder.body(), value, size);
	}

	// ---- Reply reading ---------------------------------------------------------------------------

	/// A decoded reply: the envelope, the verb, and the fields as name/value pairs.
	struct Reply {
		bool present = false;
		int32_t target = 0;
		int32_t source = 0;
		char verb[32] = {};
		uint8_t seq = 0;
		bool trailerOk = false;

		static constexpr size_t maxFields = 24;
		size_t fieldCount = 0;
		char names[maxFields][16] = {};
		uint32_t values[maxFields] = {};
		bool isBool[maxFields] = {};
		bool boolValues[maxFields] = {};
		uint8_t binary[maxFields][64] = {};
		uint8_t binaryLength[maxFields] = {};
		char strings[maxFields][48] = {};
		bool isString[maxFields] = {};

		bool has(const char * name) const
		{
			return this->indexOf(name) < this->fieldCount;
		}

		size_t indexOf(const char * name) const
		{
			for(size_t index = 0; index < this->fieldCount; index++) {
				if(strcmp(this->names[index], name) == 0) {
					return index;
				}
			}
			return this->fieldCount;
		}

		uint32_t uintAt(const char * name) const
		{
			const size_t index = this->indexOf(name);
			return index < this->fieldCount ? this->values[index] : 0;
		}

		bool boolAt(const char * name) const
		{
			const size_t index = this->indexOf(name);
			return index < this->fieldCount && this->boolValues[index];
		}

		const char * stringAt(const char * name) const
		{
			const size_t index = this->indexOf(name);
			return index < this->fieldCount ? this->strings[index] : "";
		}

		const uint8_t * binaryAt(const char * name, uint8_t & length) const
		{
			const size_t index = this->indexOf(name);
			if(index >= this->fieldCount) {
				length = 0;
				return nullptr;
			}
			length = this->binaryLength[index];
			return this->binary[index];
		}
	};

	/// Decode the next reply out of a stream's outbox, with the real decoder.
	Reply readReply(DuplexStream & stream);

} // namespace bltest
