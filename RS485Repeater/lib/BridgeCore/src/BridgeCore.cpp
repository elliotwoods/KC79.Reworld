#include "BridgeCore.h"

#include <cstring>
#include <limits>

namespace repeater {
namespace {

bool cobsDecode(const uint8_t* encoded, size_t encodedSize, uint8_t* output,
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

bool stringEquals(const uint8_t* value, uint32_t length, const char* expected) {
    const size_t expectedLength = std::strlen(expected);
    return length == expectedLength && std::memcmp(value, expected, length) == 0;
}

bool inspectKeyframe(MsgpackCursor& cursor, uint64_t& start, uint64_t& count) {
    uint32_t fields;
    if(!cursor.readMapSize(fields)) return false;
    bool haveStart = false;
    bool haveValues = false;
    for(uint32_t i = 0; i < fields; ++i) {
        const uint8_t* key;
        uint32_t keyLength;
        if(!cursor.readString(key, keyLength)) return false;
        if(stringEquals(key, keyLength, "startIndex")) {
            int64_t value;
            if(!cursor.readInteger(value) || value < 1) return false;
            start = static_cast<uint64_t>(value);
            haveStart = true;
        }
        else if(stringEquals(key, keyLength, "values")) {
            uint32_t values;
            if(!cursor.readArraySize(values)) return false;
            count = values;
            for(uint32_t j = 0; j < values; ++j) if(!cursor.skipValue()) return false;
            haveValues = true;
        }
        else if(!cursor.skipValue()) return false;
    }
    return haveStart && haveValues && count > 0;
}

} // namespace

FrameRouter::FrameRouter(uint32_t idleTimeoutUs) : idleTimeoutUs_(idleTimeoutUs) { }

void FrameRouter::ingest(Side source, const uint8_t* bytes, size_t count, uint32_t nowUs) {
    if(source == Side::None || bytes == nullptr) return;
    for(size_t i = 0; i < count; ++i) ingestByte(source, bytes[i], nowUs);
}

void FrameRouter::ingestByte(Side source, uint8_t value, uint32_t nowUs) {
    auto& acc = accumulator(source);
    auto& dir = direction(source);
    dir.rxBytes++;
    acc.lastByteUs = nowUs;

    if(acc.discardingOversize) {
        if(value == 0) {
            acc.discardingOversize = false;
            acc.size = 0;
        }
        return;
    }
    if(acc.size >= acc.data.size()) {
        dir.oversizedFrames++;
        acc.size = 0;
        acc.discardingOversize = value != 0;
        return;
    }
    acc.data[acc.size++] = value;
    if(value == 0) finishFrame(source);
}

void FrameRouter::finishFrame(Side source) {
    auto& acc = accumulator(source);
    direction(source).receivedFrames++;
    if(shouldForward(source, acc.data.data(), acc.size)) enqueue(source, acc.data.data(), acc.size);
    acc.size = 0;
}

bool FrameRouter::enqueue(Side source, const uint8_t* data, size_t size) {
    auto& q = queue(source);
    auto& dir = direction(source);
    if(q.count >= q.frames.size()) {
        dir.queueDrops++;
        return false;
    }
    auto& frame = q.frames[(q.head + q.count) % q.frames.size()];
    std::memcpy(frame.data.data(), data, size);
    frame.size = size;
    q.count++;
    if(q.count > dir.queueHighWater) dir.queueHighWater = static_cast<uint32_t>(q.count);
    return true;
}

void FrameRouter::expireIncomplete(uint32_t nowUs) {
    for(Side side : {Side::One, Side::Two}) {
        auto& acc = accumulator(side);
        if((acc.size > 0 || acc.discardingOversize)
            && static_cast<uint32_t>(nowUs - acc.lastByteUs) >= idleTimeoutUs_) {
            if(!acc.discardingOversize) direction(side).incompleteFrames++;
            acc.size = 0;
            acc.discardingOversize = false;
        }
    }
}

bool FrameRouter::nextFrame(FrameView& view) {
    const auto available = [this](Side source) {
        const Side destination = source == Side::One ? Side::Two : Side::One;
        return queue(source).count > 0 && accumulator(destination).size == 0
            && !accumulator(destination).discardingOversize;
    };
    Side selected = Side::None;
    if(available(Side::One) && available(Side::Two)) selected = lastDequeued_ == Side::One ? Side::Two : Side::One;
    else if(available(Side::One)) selected = Side::One;
    else if(available(Side::Two)) selected = Side::Two;
    if(selected == Side::None) return false;

    const auto& q = queue(selected);
    const auto& frame = q.frames[q.head];
    view = FrameView{selected, frame.data.data(), frame.size};
    return true;
}

void FrameRouter::completeTransmission(Side source, bool success) {
    if(source == Side::None) {
        stats_.txErrors++;
        return;
    }
    auto& q = queue(source);
    if(q.count == 0) {
        stats_.txErrors++;
        return;
    }
    const auto size = q.frames[q.head].size;
    if(success) {
        direction(source).forwardedBytes += size;
        direction(source).forwardedFrames++;
    }
    else stats_.txErrors++;
    q.head = (q.head + 1) % q.frames.size();
    q.count--;
    lastDequeued_ = source;
}

bool FrameRouter::shouldForward(Side source, const uint8_t* data, size_t size) {
    const EnvelopeInfo envelope = inspectEnvelope(data, size);
    if(!envelope.valid || envelope.parseError) stats_.parseErrors++;
    if(source == Side::Two) {
        if(envelope.valid) observeLocalReply(envelope);
        return true;
    }
    if(!envelope.valid) {
        return true;
    }
    if(envelope.target == 0) {
        stats_.filteredHostFrames++;
        return false;
    }
    if(routingMode_ != RoutingMode::Filtered) return true;

    const uint64_t rangeStart = localRangeStart_;
    const uint64_t rangeEnd = rangeStart + 8;
    if(envelope.target > 0 && envelope.target <= 127) {
        const bool local = static_cast<uint64_t>(envelope.target) >= rangeStart
            && static_cast<uint64_t>(envelope.target) <= rangeEnd;
        if(!local) stats_.filteredUnicasts++;
        return local;
    }
    if(envelope.target == -1 && envelope.isKeyframe) {
        const uint64_t frameEnd = envelope.keyframeCount - 1
            > std::numeric_limits<uint64_t>::max() - envelope.keyframeStart
            ? std::numeric_limits<uint64_t>::max()
            : envelope.keyframeStart + envelope.keyframeCount - 1;
        const bool intersects = envelope.keyframeStart <= rangeEnd && frameEnd >= rangeStart;
        if(!intersects) stats_.filteredKeyframes++;
        return intersects;
    }
    return true;
}

FrameRouter::EnvelopeInfo FrameRouter::inspectEnvelope(const uint8_t* data, size_t size) {
    EnvelopeInfo result;
    if(size < 2 || data[size - 1] != 0) return result;
    size_t decodedSize = 0;
    if(!cobsDecode(data, size - 1, decodeBuffer_.data(), decodeBuffer_.size(), decodedSize)) return result;

    MsgpackCursor cursor(decodeBuffer_.data(), decodedSize);
    uint32_t envelopeSize;
    if(!cursor.readArraySize(envelopeSize) || envelopeSize < 3
        || !cursor.readInteger(result.target) || !cursor.readInteger(result.source)) return result;

    uint32_t bodyFields;
    MsgpackCursor bodyCursor = cursor;
    if(!bodyCursor.readMapSize(bodyFields)) {
        if(!cursor.skipValue()) return result;
        result.valid = true;
        return result;
    }
    cursor = bodyCursor;
    for(uint32_t i = 0; i < bodyFields; ++i) {
        const uint8_t* key;
        uint32_t keyLength;
        MsgpackCursor keyCursor = cursor;
        if(keyCursor.readString(key, keyLength)) {
            cursor = keyCursor;
        }
        else {
            if(!cursor.skipValue() || !cursor.skipValue()) return result;
            continue;
        }
        if(stringEquals(key, keyLength, "keyframe")) {
            result.isKeyframe = inspectKeyframe(cursor, result.keyframeStart, result.keyframeCount);
            if(!result.isKeyframe) {
                result.valid = true;
                result.parseError = true;
                return result;
            }
        }
        else if(!cursor.skipValue()) return result;
    }
    result.valid = true;
    return result;
}

void FrameRouter::observeLocalReply(const EnvelopeInfo& envelope) {
    if(envelope.target != 0 || envelope.source < 1 || envelope.source > 127) return;
    if(envelope.source > 54) {
        if(routingMode_ != RoutingMode::Conflict) {
            localRangeStart_ = 0;
            routingMode_ = RoutingMode::Conflict;
            stats_.topologyConflicts++;
        }
        return;
    }
    const uint8_t rangeStart = static_cast<uint8_t>(((envelope.source - 1) / 9) * 9 + 1);
    if(routingMode_ == RoutingMode::Transparent) {
        localRangeStart_ = rangeStart;
        routingMode_ = RoutingMode::Filtered;
    }
    else if(routingMode_ == RoutingMode::Filtered && rangeStart != localRangeStart_) {
        localRangeStart_ = 0;
        routingMode_ = RoutingMode::Conflict;
        stats_.topologyConflicts++;
    }
}

void FrameRouter::relearn() {
    routingMode_ = RoutingMode::Transparent;
    localRangeStart_ = 0;
}

void FrameRouter::resetStats() { stats_ = RouterStats{}; }

size_t FrameRouter::queueDepth(Side source) const { return source == Side::None ? 0 : queue(source).count; }

FrameRouter::Accumulator& FrameRouter::accumulator(Side source) {
    return source == Side::One ? sideOneAccumulator_ : sideTwoAccumulator_;
}

const FrameRouter::Accumulator& FrameRouter::accumulator(Side source) const {
    return source == Side::One ? sideOneAccumulator_ : sideTwoAccumulator_;
}

FrameRouter::FrameQueue& FrameRouter::queue(Side source) {
    return source == Side::One ? oneToTwoQueue_ : twoToOneQueue_;
}

const FrameRouter::FrameQueue& FrameRouter::queue(Side source) const {
    return source == Side::One ? oneToTwoQueue_ : twoToOneQueue_;
}

DirectionStats& FrameRouter::direction(Side source) {
    return source == Side::One ? stats_.oneToTwo : stats_.twoToOne;
}

const char* routingModeName(RoutingMode mode) {
    switch(mode) {
    case RoutingMode::Transparent: return "transparent";
    case RoutingMode::Filtered: return "filtered";
    case RoutingMode::Conflict: return "conflict";
    }
    return "unknown";
}

} // namespace repeater
