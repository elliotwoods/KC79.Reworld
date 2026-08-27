#include "ControlPlane.h"

#include <cstring>

namespace repeater {
namespace {

struct VerbName {
    ControlVerb verb;
    const char* name;
};

// Spelled out rather than abbreviated. The only high-volume verb is `ota-data`,
// and at 617 chunks the difference against a terse spelling is under two kilobytes
// of a three-hundred kilobyte transfer.
constexpr VerbName VERB_NAMES[] = {
    {ControlVerb::Status, "status"},
    {ControlVerb::Relearn, "relearn"},
    {ControlVerb::ResetCounters, "reset-counters"},
    {ControlVerb::Reboot, "reboot"},
    {ControlVerb::SetIndex, "set-index"},
    {ControlVerb::SnapshotStart, "snap-start"},
    {ControlVerb::SnapshotRead, "snap-read"},
    {ControlVerb::OtaBegin, "ota-begin"},
    {ControlVerb::OtaData, "ota-data"},
    {ControlVerb::OtaMap, "ota-map"},
    {ControlVerb::OtaEnd, "ota-end"},
    {ControlVerb::OtaBoot, "ota-boot"},
    {ControlVerb::OtaConfirm, "ota-confirm"},
    {ControlVerb::OtaAbort, "ota-abort"},
    {ControlVerb::SetPolarity, "set-polarity"},
};

ControlVerb verbFromName(const uint8_t* name, uint32_t length) {
    for(const auto& entry : VERB_NAMES) {
        if(wire::stringEquals(name, length, entry.name)) return entry.verb;
    }
    return ControlVerb::Unknown;
}

} // namespace

const char* controlVerbName(ControlVerb verb) {
    for(const auto& entry : VERB_NAMES) {
        if(entry.verb == verb) return entry.name;
    }
    return "unknown";
}

bool controlVerbRepliesToUnicastOnly(ControlVerb verb) {
    switch(verb) {
    // These solicit no reply, so every repeater may act on one broadcast.
    case ControlVerb::SnapshotStart:
    case ControlVerb::OtaData:
    case ControlVerb::OtaBoot:
    case ControlVerb::OtaAbort:
        return false;
    default:
        return true;
    }
}

ControlPlane::ControlPlane() = default;

void ControlPlane::setIdentity(int8_t index, const uint8_t mac[MAC_BYTES]) {
    index_ = (index >= 1 && index <= static_cast<int8_t>(REPEATER_COUNT)) ? index : 0;
    if(mac != nullptr) std::memcpy(mac_, mac, MAC_BYTES);
}

ControlRequest ControlPlane::parse(const uint8_t* frame, size_t size) {
    ControlRequest request;
    if(frame == nullptr || size < 2 || frame[size - 1] != 0) return request;

    size_t decodedSize = 0;
    if(!wire::cobsDecode(frame, size - 1, decodeBuffer_, sizeof(decodeBuffer_), decodedSize)) return request;

    wire::MsgpackCursor cursor(decodeBuffer_, decodedSize);
    uint32_t envelopeSize = 0;
    int64_t target = 0;
    int64_t source = 0;
    if(!cursor.readArraySize(envelopeSize) || envelopeSize < 3
        || !cursor.readInteger(target) || !cursor.readInteger(source)) {
        return request;
    }
    if(target != 0) return request;

    uint32_t bodyFields = 0;
    if(!cursor.readMapSize(bodyFields) || bodyFields != 1) return request;

    const uint8_t* bodyKey = nullptr;
    uint32_t bodyKeyLength = 0;
    if(!cursor.readString(bodyKey, bodyKeyLength)) return request;
    if(!wire::stringEquals(bodyKey, bodyKeyLength, "rq")) return request;

    uint32_t fields = 0;
    if(!cursor.readMapSize(fields)) return request;

    bool haveAddress = false;
    bool matched = false;
    bool broadcast = false;
    for(uint32_t i = 0; i < fields; ++i) {
        const uint8_t* key = nullptr;
        uint32_t keyLength = 0;
        if(!cursor.readString(key, keyLength)) return request;

        if(wire::stringEquals(key, keyLength, "a")) {
            wire::MsgpackCursor probe = cursor;
            int64_t address = 0;
            const uint8_t* mac = nullptr;
            uint32_t macLength = 0;
            if(probe.readInteger(address)) {
                cursor = probe;
                broadcast = address == REPEATER_ALL;
                matched = broadcast
                    || (index_ != 0 && address == repeaterAddress(index_));
                haveAddress = true;
            }
            else {
                probe = cursor;
                if(!probe.readBinary(mac, macLength)) return request;
                cursor = probe;
                // A MAC always reaches the unit, even one whose index is unset or
                // wrong. That is the escape hatch for a repeater with a dead branch.
                matched = macLength == MAC_BYTES && std::memcmp(mac, mac_, MAC_BYTES) == 0;
                haveAddress = true;
            }
        }
        else if(wire::stringEquals(key, keyLength, "q")) {
            const uint8_t* name = nullptr;
            uint32_t nameLength = 0;
            if(!cursor.readString(name, nameLength)) return request;
            request.verb = verbFromName(name, nameLength);
        }
        else if(wire::stringEquals(key, keyLength, "v")) {
            const size_t start = cursor.offset();
            if(!cursor.skipValue()) return request;
            request.payload = decodeBuffer_ + start;
            request.payloadSize = cursor.offset() - start;
        }
        else if(!cursor.skipValue()) return request;
    }

    if(!haveAddress) return request;
    request.valid = true;
    request.broadcast = broadcast;
    // A reply-bearing verb sent to every repeater at once would put six answers on
    // the wire together, so it is recognised but not treated as ours to act on.
    request.addressedToUs = matched
        && !(broadcast && controlVerbRepliesToUnicastOnly(request.verb));
    return request;
}

wire::MsgpackWriter& ControlPlane::beginReply(ControlVerb verb, bool ok, bool withPayload) {
    replyWriter_ = wire::MsgpackWriter(replyBuffer_, sizeof(replyBuffer_));
    replyOpen_ = true;

    // An unprovisioned unit answers as REPEATER_ALL. Only a MAC-addressed request
    // can reach one, and a MAC names exactly one unit, so there is nothing for that
    // source to collide with — and it lets a fresh repeater be discovered at all.
    const int8_t replySource = address() != 0 ? address() : REPEATER_ALL;

    replyWriter_.arrayHeader(3);
    replyWriter_.integer(0);                 // target: the host
    replyWriter_.integer(replySource);       // source: this repeater
    replyWriter_.mapHeader(1);
    replyWriter_.key("rr");
    replyWriter_.mapHeader(withPayload ? 4 : 3);
    replyWriter_.key("a");
    replyWriter_.integer(replySource);
    replyWriter_.key("q");
    replyWriter_.string(controlVerbName(verb));
    replyWriter_.key("ok");
    replyWriter_.boolean(ok);
    if(withPayload) replyWriter_.key("v");
    return replyWriter_;
}

size_t ControlPlane::finishReply(uint8_t* out, size_t capacity) {
    if(!replyOpen_ || out == nullptr) return 0;
    replyOpen_ = false;
    if(!replyWriter_.ok()) return 0;
    return wire::cobsEncodeFrame(replyWriter_.data(), replyWriter_.size(), out, capacity);
}

} // namespace repeater
