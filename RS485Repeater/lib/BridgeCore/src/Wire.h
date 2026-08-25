#pragma once

/// COBS framing, MessagePack reading and writing, and the CRC the rest of the
/// system already uses. Header-only so both the frame router and the control
/// plane share exactly one implementation, and so all of it is reachable from
/// the native test environment.
///
/// The MessagePack integer encoding deliberately mirrors `dump_int` in
/// `RouterRS/crates/router-proto/src/value.rs`: non-negative values route
/// through the unsigned families, negatives take the smallest signed form.

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <limits>

namespace repeater {
namespace wire {

inline bool cobsDecode(const uint8_t* encoded, size_t encodedSize, uint8_t* output,
    size_t outputCapacity, size_t& outputSize) {
    outputSize = 0;
    size_t read = 0;
    while(read < encodedSize) {
        const uint8_t code = encoded[read++];
        if(code == 0) return false;
        const size_t copyCount = static_cast<size_t>(code - 1);
        if(copyCount > encodedSize - read || copyCount > outputCapacity - outputSize) return false;
        if(copyCount > 0) {
            std::memcpy(output + outputSize, encoded + read, copyCount);
            read += copyCount;
            outputSize += copyCount;
        }
        if(code != 0xFF && read < encodedSize) {
            if(outputSize >= outputCapacity) return false;
            output[outputSize++] = 0;
        }
    }
    return encodedSize > 0;
}

/// Encodes `input` and appends the terminating zero, producing a complete frame.
/// Returns 0 if it would not fit in `outputCapacity`.
inline size_t cobsEncodeFrame(const uint8_t* input, size_t inputSize, uint8_t* output,
    size_t outputCapacity) {
    if(outputCapacity < inputSize + inputSize / 254 + 2) return 0;
    size_t codeIndex = 0;
    size_t written = 1; // the first code byte is back-filled
    uint8_t code = 1;
    for(size_t i = 0; i < inputSize; ++i) {
        if(input[i] == 0) {
            output[codeIndex] = code;
            codeIndex = written++;
            code = 1;
        }
        else {
            output[written++] = input[i];
            if(++code == 0xFF) {
                output[codeIndex] = code;
                codeIndex = written++;
                code = 1;
            }
        }
    }
    output[codeIndex] = code;
    output[written++] = 0;
    return written;
}

/// CRC-16/CCITT-FALSE: poly 0x1021, init 0xFFFF, no reflection, xorout 0x0000.
/// The definition pinned in `protocol-hardening.md`; CRC of "123456789" is 0x29B1.
inline uint16_t crc16CcittFalse(const uint8_t* data, size_t size, uint16_t seed = 0xFFFF) {
    uint16_t crc = seed;
    for(size_t i = 0; i < size; ++i) {
        crc ^= static_cast<uint16_t>(data[i]) << 8;
        for(int bit = 0; bit < 8; ++bit) {
            crc = (crc & 0x8000) ? static_cast<uint16_t>((crc << 1) ^ 0x1021) : static_cast<uint16_t>(crc << 1);
        }
    }
    return crc;
}

inline bool stringEquals(const uint8_t* value, uint32_t length, const char* expected) {
    const size_t expectedLength = std::strlen(expected);
    return length == expectedLength && std::memcmp(value, expected, length) == 0;
}

/// Non-allocating forward reader. Every accessor leaves the cursor unmoved on
/// failure only insofar as the caller copies the cursor before a speculative
/// read; that copy-and-commit idiom is used throughout.
class MsgpackCursor {
public:
    MsgpackCursor(const uint8_t* data, size_t size) : data_(data), size_(size) { }

    bool readArraySize(uint32_t& count) {
        uint8_t marker;
        if(!take(marker)) return false;
        if((marker & 0xF0) == 0x90) {
            count = marker & 0x0F;
            return true;
        }
        if(marker == 0xDC) return readUnsignedWidth(2, count);
        if(marker == 0xDD) return readUnsignedWidth(4, count);
        return false;
    }

    bool readMapSize(uint32_t& count) {
        uint8_t marker;
        if(!take(marker)) return false;
        if((marker & 0xF0) == 0x80) {
            count = marker & 0x0F;
            return true;
        }
        if(marker == 0xDE) return readUnsignedWidth(2, count);
        if(marker == 0xDF) return readUnsignedWidth(4, count);
        return false;
    }

    bool readInteger(int64_t& value) {
        uint8_t marker;
        if(!take(marker)) return false;
        if(marker <= 0x7F) {
            value = marker;
            return true;
        }
        if(marker >= 0xE0) {
            value = static_cast<int8_t>(marker);
            return true;
        }
        uint64_t raw = 0;
        switch(marker) {
        case 0xCC: if(!readUnsigned(1, raw)) return false; value = static_cast<int64_t>(raw); return true;
        case 0xCD: if(!readUnsigned(2, raw)) return false; value = static_cast<int64_t>(raw); return true;
        case 0xCE: if(!readUnsigned(4, raw)) return false; value = static_cast<int64_t>(raw); return true;
        case 0xCF:
            if(!readUnsigned(8, raw) || raw > static_cast<uint64_t>(std::numeric_limits<int64_t>::max())) return false;
            value = static_cast<int64_t>(raw);
            return true;
        case 0xD0: if(!readUnsigned(1, raw)) return false; value = static_cast<int8_t>(raw); return true;
        case 0xD1: if(!readUnsigned(2, raw)) return false; value = static_cast<int16_t>(raw); return true;
        case 0xD2: if(!readUnsigned(4, raw)) return false; value = static_cast<int32_t>(raw); return true;
        case 0xD3: if(!readUnsigned(8, raw)) return false; value = static_cast<int64_t>(raw); return true;
        default: return false;
        }
    }

    bool readString(const uint8_t*& value, uint32_t& length) {
        uint8_t marker;
        if(!take(marker)) return false;
        if((marker & 0xE0) == 0xA0) length = marker & 0x1F;
        else if(marker == 0xD9) { if(!readUnsignedWidth(1, length)) return false; }
        else if(marker == 0xDA) { if(!readUnsignedWidth(2, length)) return false; }
        else if(marker == 0xDB) { if(!readUnsignedWidth(4, length)) return false; }
        else return false;
        if(length > size_ - offset_) return false;
        value = data_ + offset_;
        offset_ += length;
        return true;
    }

    bool readBinary(const uint8_t*& value, uint32_t& length) {
        uint8_t marker;
        if(!take(marker)) return false;
        if(marker == 0xC4) { if(!readUnsignedWidth(1, length)) return false; }
        else if(marker == 0xC5) { if(!readUnsignedWidth(2, length)) return false; }
        else if(marker == 0xC6) { if(!readUnsignedWidth(4, length)) return false; }
        else return false;
        if(length > size_ - offset_) return false;
        value = data_ + offset_;
        offset_ += length;
        return true;
    }

    bool readNil() {
        uint8_t marker;
        if(!take(marker)) return false;
        return marker == 0xC0;
    }

    /// How far the cursor has advanced. Two offsets either side of a `skipValue()`
    /// delimit that value's raw bytes.
    size_t offset() const { return offset_; }

    bool skipValue(uint8_t depth = 0) {
        if(depth > 24 || offset_ >= size_) return false;
        const uint8_t marker = data_[offset_++];
        if(marker <= 0x7F || marker >= 0xE0 || marker == 0xC0 || marker == 0xC2 || marker == 0xC3) return true;
        if((marker & 0xE0) == 0xA0) return skipBytes(marker & 0x1F);
        if((marker & 0xF0) == 0x90) return skipMany(marker & 0x0F, depth);
        if((marker & 0xF0) == 0x80) return skipMany((marker & 0x0F) * 2u, depth);

        uint64_t length = 0;
        switch(marker) {
        case 0xC4: if(!readUnsigned(1, length)) return false; return skipBytes(length);
        case 0xC5: if(!readUnsigned(2, length)) return false; return skipBytes(length);
        case 0xC6: if(!readUnsigned(4, length)) return false; return skipBytes(length);
        case 0xCA: return skipBytes(4);
        case 0xCB: return skipBytes(8);
        case 0xCC: case 0xD0: return skipBytes(1);
        case 0xCD: case 0xD1: return skipBytes(2);
        case 0xCE: case 0xD2: return skipBytes(4);
        case 0xCF: case 0xD3: return skipBytes(8);
        case 0xD4: return skipBytes(2);
        case 0xD5: return skipBytes(3);
        case 0xD6: return skipBytes(5);
        case 0xD7: return skipBytes(9);
        case 0xD8: return skipBytes(17);
        case 0xD9: if(!readUnsigned(1, length)) return false; return skipBytes(length);
        case 0xDA: if(!readUnsigned(2, length)) return false; return skipBytes(length);
        case 0xDB: if(!readUnsigned(4, length)) return false; return skipBytes(length);
        case 0xDC: if(!readUnsigned(2, length)) return false; return skipMany(length, depth);
        case 0xDD: if(!readUnsigned(4, length)) return false; return skipMany(length, depth);
        case 0xDE: if(!readUnsigned(2, length)) return false; return skipMany(length * 2u, depth);
        case 0xDF: if(!readUnsigned(4, length)) return false; return skipMany(length * 2u, depth);
        case 0xC7: if(!readUnsigned(1, length)) return false; return skipBytes(length + 1);
        case 0xC8: if(!readUnsigned(2, length)) return false; return skipBytes(length + 1);
        case 0xC9: if(!readUnsigned(4, length)) return false; return skipBytes(length + 1);
        default: return false;
        }
    }

private:
    bool take(uint8_t& value) {
        if(offset_ >= size_) return false;
        value = data_[offset_++];
        return true;
    }

    bool readUnsigned(size_t width, uint64_t& value) {
        if(width > size_ - offset_) return false;
        value = 0;
        for(size_t i = 0; i < width; ++i) value = (value << 8) | data_[offset_++];
        return true;
    }

    bool readUnsignedWidth(size_t width, uint32_t& value) {
        uint64_t raw;
        if(!readUnsigned(width, raw) || raw > std::numeric_limits<uint32_t>::max()) return false;
        value = static_cast<uint32_t>(raw);
        return true;
    }

    bool skipBytes(uint64_t count) {
        if(count > size_ - offset_) return false;
        offset_ += static_cast<size_t>(count);
        return true;
    }

    bool skipMany(uint64_t count, uint8_t depth) {
        if(count > size_) return false;
        for(uint64_t i = 0; i < count; ++i) {
            if(!skipValue(static_cast<uint8_t>(depth + 1))) return false;
        }
        return true;
    }

    const uint8_t* data_;
    size_t size_;
    size_t offset_ = 0;
};

/// Writes into a caller-owned buffer. Overflow is latched rather than reported
/// per call, so a run of writes can be checked once with `ok()`.
class MsgpackWriter {
public:
    MsgpackWriter(uint8_t* data, size_t capacity) : data_(data), capacity_(capacity) { }

    void arrayHeader(uint32_t count) {
        if(count < 16) put(static_cast<uint8_t>(0x90 | count));
        else if(count <= 0xFFFF) { put(0xDC); putBig(count, 2); }
        else { put(0xDD); putBig(count, 4); }
    }

    void mapHeader(uint32_t count) {
        if(count < 16) put(static_cast<uint8_t>(0x80 | count));
        else if(count <= 0xFFFF) { put(0xDE); putBig(count, 2); }
        else { put(0xDF); putBig(count, 4); }
    }

    void nil() { put(0xC0); }

    void boolean(bool value) { put(value ? 0xC3 : 0xC2); }

    void uinteger(uint64_t value) {
        if(value < 0x80) put(static_cast<uint8_t>(value));
        else if(value <= 0xFF) { put(0xCC); putBig(value, 1); }
        else if(value <= 0xFFFF) { put(0xCD); putBig(value, 2); }
        else if(value <= 0xFFFFFFFFull) { put(0xCE); putBig(value, 4); }
        else { put(0xCF); putBig(value, 8); }
    }

    void integer(int64_t value) {
        if(value >= 0) { uinteger(static_cast<uint64_t>(value)); return; }
        if(value >= -32) { put(static_cast<uint8_t>(static_cast<int8_t>(value))); return; }
        if(value >= -128) { put(0xD0); put(static_cast<uint8_t>(static_cast<int8_t>(value))); return; }
        if(value >= -32768) { put(0xD1); putBig(static_cast<uint64_t>(static_cast<uint16_t>(value)), 2); return; }
        if(value >= -2147483647LL - 1) { put(0xD2); putBig(static_cast<uint64_t>(static_cast<uint32_t>(value)), 4); return; }
        put(0xD3);
        putBig(static_cast<uint64_t>(value), 8);
    }

    void string(const char* value) {
        const size_t length = std::strlen(value);
        if(length < 32) put(static_cast<uint8_t>(0xA0 | length));
        else if(length <= 0xFF) { put(0xD9); putBig(length, 1); }
        else if(length <= 0xFFFF) { put(0xDA); putBig(length, 2); }
        else { put(0xDB); putBig(length, 4); }
        raw(reinterpret_cast<const uint8_t*>(value), length);
    }

    void binary(const uint8_t* value, size_t length) {
        if(length <= 0xFF) { put(0xC4); putBig(length, 1); }
        else if(length <= 0xFFFF) { put(0xC5); putBig(length, 2); }
        else { put(0xC6); putBig(length, 4); }
        raw(value, length);
    }

    void raw(const uint8_t* value, size_t length) {
        if(overflow_ || length > capacity_ - size_) {
            overflow_ = true;
            return;
        }
        std::memcpy(data_ + size_, value, length);
        size_ += length;
    }

    /// `{"<key>": ` — a convenience for the single-key maps this protocol uses.
    void key(const char* name) { string(name); }

    bool ok() const { return !overflow_; }
    size_t size() const { return size_; }
    const uint8_t* data() const { return data_; }

private:
    void put(uint8_t value) {
        if(overflow_ || size_ >= capacity_) {
            overflow_ = true;
            return;
        }
        data_[size_++] = value;
    }

    void putBig(uint64_t value, size_t width) {
        for(size_t i = 0; i < width; ++i) {
            put(static_cast<uint8_t>((value >> ((width - 1 - i) * 8)) & 0xFF));
        }
    }

    uint8_t* data_;
    size_t capacity_;
    size_t size_ = 0;
    bool overflow_ = false;
};

} // namespace wire
} // namespace repeater
