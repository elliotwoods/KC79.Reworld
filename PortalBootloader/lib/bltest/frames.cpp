#include "frames.hpp"

namespace bltest {
	namespace {
		/// Reads out of a `DuplexStream`'s outbox so the real COBS decoder can be pointed at it.
		class OutboxStream : public msgpack::Stream {
		public:
			OutboxStream(const uint8_t * data, size_t length)
			: data(data)
			, length(length)
			{
			}

			int available() override { return (int) (this->length - this->position); }
			int read() override
			{
				return this->position < this->length ? (int) this->data[this->position++] : -1;
			}
			int peek() override
			{
				return this->position < this->length ? (int) this->data[this->position] : -1;
			}
			size_t write(uint8_t) override { return 0; }
			size_t write(const uint8_t *, size_t size) override { return size; }
			void flush() override {}

		private:
			const uint8_t * data;
			size_t length;
			size_t position = 0;
		};

		void copyName(char * out, size_t outSize, const char * in, uint8_t size)
		{
			size_t take = size;
			if(take >= outSize) {
				take = outSize - 1;
			}
			memcpy(out, in, take);
			out[take] = '\0';
		}

		/// One field of the reply's inner map.
		bool readField(msgpack::COBSRWStream & cobs, Reply & reply)
		{
			if(reply.fieldCount >= Reply::maxFields) {
				return false;
			}

			char name[32];
			uint8_t nameSize = 0;
			if(!msgpack::readString5(cobs, name, (uint8_t) sizeof(name), nameSize)) {
				return false;
			}
			const size_t slot = reply.fieldCount;
			copyName(reply.names[slot], sizeof(reply.names[slot]), name, nameSize);

			msgpack::DataType type;
			if(!msgpack::getNextDataType(cobs, type)) {
				return false;
			}

			if(type == msgpack::DataType::Bool) {
				bool value = false;
				if(!msgpack::readBool(cobs, value)) {
					return false;
				}
				reply.isBool[slot] = true;
				reply.boolValues[slot] = value;
			}
			else if(type == msgpack::DataType::String5 || type == msgpack::DataType::String8) {
				char value[64];
				uint8_t valueSize = 0;
				if(!msgpack::readString5(cobs, value, (uint8_t) sizeof(value), valueSize)) {
					return false;
				}
				reply.isString[slot] = true;
				copyName(reply.strings[slot], sizeof(reply.strings[slot]), value, valueSize);
			}
			else if(type == msgpack::DataType::Binary8 || type == msgpack::DataType::Binary16) {
				uint16_t declared = 0;
				if(!msgpack::readBinarySize(cobs, declared)) {
					return false;
				}
				uint8_t take = (uint8_t) (declared > sizeof(reply.binary[slot])
					? sizeof(reply.binary[slot])
					: declared);
				cobs.readBytes((char *) reply.binary[slot], take);
				// Drain anything the fixture is too small to keep, so the parse stays in step.
				for(uint16_t index = take; index < declared; index++) {
					cobs.read();
				}
				reply.binaryLength[slot] = take;
			}
			else if(type == msgpack::DataType::Map) {
				// A nested map (`app`). Recorded as present, its own fields flattened in after it
				// so a test can read `app` and then `base`/`ver` by name.
				size_t nested = 0;
				if(!msgpack::readMapSize(cobs, nested)) {
					return false;
				}
				reply.values[slot] = (uint32_t) nested;
				reply.fieldCount++;
				for(size_t index = 0; index < nested; index++) {
					if(!readField(cobs, reply)) {
						return false;
					}
				}
				return true;
			}
			else {
				int32_t value = 0;
				if(!msgpack::readInt<int32_t>(cobs, value)) {
					return false;
				}
				reply.values[slot] = (uint32_t) value;
			}

			reply.fieldCount++;
			return true;
		}
	}

	//----------
	Reply readReply(DuplexStream & stream)
	{
		Reply reply;

		OutboxStream raw(stream.sent(), stream.sentLength());
		msgpack::COBSRWStream cobs(raw);

		if(!cobs.isStartOfIncomingPacket()) {
			cobs.nextIncomingPacket();
		}
		// Skip empty packets. The bootloader emits a bare delimiter before every reply so that a
		// half-duplex listener which latched a turn-around byte starts this frame from a known
		// boundary; two delimiters in a row are an empty packet, and any receiver has to step over
		// one. Bounded, so a stream of nothing but delimiters cannot spin here.
		for(int skips = 0; skips < 4 && cobs.available() <= 0; skips++) {
			cobs.nextIncomingPacket();
		}
		if(cobs.available() <= 0) {
			return reply;
		}

		size_t arraySize = 0;
		if(!msgpack::readArraySize(cobs, arraySize) || arraySize < 3) {
			return reply;
		}
		if(!msgpack::readInt<int32_t>(cobs, reply.target)
			|| !msgpack::readInt<int32_t>(cobs, reply.source)) {
			return reply;
		}

		size_t outer = 0;
		if(!msgpack::readMapSize(cobs, outer) || outer != 1) {
			return reply;
		}
		char key[8];
		uint8_t keySize = 0;
		if(!msgpack::readString5(cobs, key, (uint8_t) sizeof(key), keySize)) {
			return reply;
		}
		if(keySize != 2 || memcmp(key, "bl", 2) != 0) {
			return reply;
		}

		size_t fields = 0;
		if(!msgpack::readMapSize(cobs, fields) || fields < 1) {
			return reply;
		}

		// The first field is always the verb.
		char qName[8];
		uint8_t qSize = 0;
		if(!msgpack::readString5(cobs, qName, (uint8_t) sizeof(qName), qSize)) {
			return reply;
		}
		uint8_t verbSize = 0;
		if(!msgpack::readString5(cobs, reply.verb, (uint8_t) sizeof(reply.verb), verbSize)) {
			return reply;
		}
		reply.verb[verbSize] = '\0';

		for(size_t index = 1; index < fields; index++) {
			if(!readField(cobs, reply)) {
				return reply;
			}
		}

		if(arraySize >= 5) {
			int32_t seq = 0;
			if(!msgpack::readInt<int32_t>(cobs, seq)) {
				return reply;
			}
			const uint16_t computed = cobs.getRxRunningCRC();
			int32_t transmitted = 0;
			if(!msgpack::readInt<int32_t>(cobs, transmitted)) {
				return reply;
			}
			reply.seq = (uint8_t) seq;
			reply.trailerOk = ((uint16_t) transmitted == computed);
		}

		reply.present = true;
		return reply;
	}

} // namespace bltest
