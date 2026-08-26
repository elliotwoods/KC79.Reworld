#include "BridgeCore.h"

#include <cstring>
#include <limits>

#include "Wire.h"

namespace repeater {
namespace {

using wire::cobsDecode;
using wire::MsgpackCursor;
using wire::stringEquals;

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
    if(value == 0 && acc.size == 0) {
        // A delimiter with nothing in front of it closes nothing. One leading a frame, or two
        // in a row, is an empty COBS packet: it decodes to no bytes, and the turn-around glitch
        // on a half-duplex bus manufactures them for real.
        //
        // It used to be stored and forwarded as a one-byte frame: counted as received, counted
        // as a parse error (inspectEnvelope rejects size < 2), and relayed to the far side under
        // its OWN driver-enable interval -- which put a fresh turn-around glitch immediately in
        // front of the real frame the delimiter was there to protect. It also burned a queue slot
        // per frame, halving a depth that was sized for the twelve keyframes a host can emit
        // during a branch sweep.
        dir.emptyFrames++;
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

bool FrameRouter::destinationQuiet(Side destination) const {
    const auto& acc = accumulator(destination);
    return acc.size == 0 && !acc.discardingOversize;
}

bool FrameRouter::originate(Side destination, const uint8_t* data, size_t size) {
    if(destination == Side::None || data == nullptr || size == 0) {
        stats_.originateDrops++;
        return false;
    }
    auto& q = originateQueue(destination);
    if(size > MAX_ORIGINATED_FRAME_BYTES || q.count >= q.frames.size()) {
        stats_.originateDrops++;
        return false;
    }
    auto& frame = q.frames[(q.head + q.count) % q.frames.size()];
    std::memcpy(frame.data.data(), data, size);
    frame.size = size;
    q.count++;
    return true;
}

void FrameRouter::completeOriginated(Side destination, bool success) {
    if(destination == Side::None) {
        stats_.txErrors++;
        return;
    }
    auto& q = originateQueue(destination);
    if(q.count == 0) {
        stats_.txErrors++;
        return;
    }
    if(success) stats_.originatedFrames++;
    else stats_.txErrors++;
    q.head = (q.head + 1) % q.frames.size();
    q.count--;
    lastOriginated_ = destination;
}

size_t FrameRouter::originateDepth(Side destination) const {
    return destination == Side::None ? 0 : originateQueue(destination).count;
}

bool FrameRouter::nextFrame(FrameView& view) {
    // Locally originated frames go first. They are low-volume and time-critical
    // (branch polls, control-plane and OTA replies) whereas relayed traffic is not,
    // and the relay queues are deep enough to absorb the delay.
    const auto originatedReady = [this](Side destination) {
        return originateQueue(destination).count > 0 && destinationQuiet(destination);
    };
    Side originated = Side::None;
    if(originatedReady(Side::One) && originatedReady(Side::Two)) {
        originated = lastOriginated_ == Side::One ? Side::Two : Side::One;
    }
    else if(originatedReady(Side::One)) originated = Side::One;
    else if(originatedReady(Side::Two)) originated = Side::Two;
    if(originated != Side::None) {
        const auto& q = originateQueue(originated);
        const auto& frame = q.frames[q.head];
        view = FrameView{Side::None, originated, frame.data.data(), frame.size};
        return true;
    }

    const auto available = [this](Side source) {
        const Side destination = source == Side::One ? Side::Two : Side::One;
        return queue(source).count > 0 && destinationQuiet(destination);
    };
    Side selected = Side::None;
    if(available(Side::One) && available(Side::Two)) selected = lastDequeued_ == Side::One ? Side::Two : Side::One;
    else if(available(Side::One)) selected = Side::One;
    else if(available(Side::Two)) selected = Side::Two;
    if(selected == Side::None) return false;

    const auto& q = queue(selected);
    const auto& frame = q.frames[q.head];
    const Side destination = selected == Side::One ? Side::Two : Side::One;
    view = FrameView{selected, destination, frame.data.data(), frame.size};
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
        // A local subsystem may claim a reply it solicited itself, so a snapshot
        // sweep's own polls do not also surface upstream as loose replies.
        if(envelope.valid && innerReplyConsumer_ != nullptr) {
            const InnerFrameInfo info{envelope.target, envelope.source, envelope.isPositionReply};
            if(innerReplyConsumer_->consumeInnerReply(info, data, size)) {
                stats_.consumedInnerFrames++;
                return false;
            }
        }
        if(forwardingPaused_) {
            stats_.pausedDrops++;
            return false;
        }
        return true;
    }
    if(envelope.valid && envelope.target == 0) {
        // Repeater-plane traffic, or a reply that has come the wrong way. Classified
        // ahead of the pause so a paused repeater can still be told to resume.
        ControlDisposition disposition = ControlDisposition::NotControl;
        if(controlFrameConsumer_ != nullptr) {
            disposition = controlFrameConsumer_->consumeControlFrame(data, size);
        }
        switch(disposition) {
        case ControlDisposition::Consumed:
            stats_.controlFrames++;
            return false;
        case ControlDisposition::ConsumedAndRelay:
            stats_.controlFrames++;
            break;
        case ControlDisposition::Relay:
            stats_.relayedControlFrames++;
            break;
        case ControlDisposition::NotControl:
            // A Portal reply cannot legitimately arrive from upstream, and must never
            // reach a branch.
            stats_.filteredHostFrames++;
            return false;
        }
        // Relaying a control frame is still relaying: an update that has paused this
        // bridge cannot deliver it either, which is why a fleet update rolls from the
        // far end of the chain back towards the host.
        if(forwardingPaused_) {
            stats_.pausedDrops++;
            return false;
        }
        return true;
    }
    // Ahead of the fail-open branch below: maintenance mode has to hold for
    // undecodable traffic too, or a garbled frame would still reach the branch.
    if(forwardingPaused_) {
        stats_.pausedDrops++;
        return false;
    }
    // Everything else goes downstream, decodable or not. A panel cannot tell which of
    // the frames crossing it are for its own Portals and which are for the panels below,
    // and it does not need to: the bus is shared either way, and a Portal ignores an
    // envelope that is not addressed to it.
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
        else if(stringEquals(key, keyLength, "p")) {
            // Both the `{"p": nil}` request and the `{"p": [...]}` reply carry this key;
            // only the direction the caller sees distinguishes them.
            result.isPositionReply = true;
            if(!cursor.skipValue()) return result;
        }
        else if(!cursor.skipValue()) return result;
    }
    result.valid = true;
    return result;
}

void FrameRouter::clearLocalBlock() {
    blockState_ = BlockState::Unknown;
    localRangeStart_ = 0;
}

void FrameRouter::setLocalBlock(uint8_t rangeStart) {
    // Only a legal block start is accepted, so a corrupt stored value cannot make a panel
    // sweep nine IDs that are not its own.
    if(rangeStart == 0 || rangeStart > 118 || (rangeStart - 1) % 9 != 0) return;
    localRangeStart_ = rangeStart;
    blockState_ = BlockState::Assigned;
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

FrameRouter::OriginateQueue& FrameRouter::originateQueue(Side destination) {
    return destination == Side::One ? originateToOneQueue_ : originateToTwoQueue_;
}

const FrameRouter::OriginateQueue& FrameRouter::originateQueue(Side destination) const {
    return destination == Side::One ? originateToOneQueue_ : originateToTwoQueue_;
}

DirectionStats& FrameRouter::direction(Side source) {
    return source == Side::One ? stats_.oneToTwo : stats_.twoToOne;
}

const char* blockStateName(BlockState state) {
    switch(state) {
    case BlockState::Unknown: return "unknown";
    case BlockState::Assigned: return "assigned";
    }
    return "unknown";
}

} // namespace repeater
