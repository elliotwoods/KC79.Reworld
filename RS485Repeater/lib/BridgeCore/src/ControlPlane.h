#pragma once

/// The repeater control plane: parsing repeater-addressed requests off the outer
/// bus and framing the replies.
///
/// Wire shape:
///
///     host  -> repeater : [0, 0,  {"rq": {"a": <addr>, "q": "<verb>", "v": <payload>}}]
///     repeater -> host  : [0, <addr>, {"rr": {"a": <addr>, "q": "<verb>", "ok": <bool>, "v": <payload>}}]
///
/// The envelope target is 0 rather than the repeater address. That is deliberate:
/// a repeater running v2.2.0 drops every side-1 frame with target 0 before it
/// consults its routing mode, so it ignores control traffic instead of relaying
/// several hundred kilobytes onto nine Portals. Portals and the frozen STM32
/// bootloader never see these frames at all.
///
/// `a` is the repeater address (`REPEATER_ALL`, or `-3`..`-8` for repeaters 1..6),
/// or a six-byte `bin` MAC address, which addresses a unit whose index is unset or
/// wrong. `v` is optional.

#include <cstddef>
#include <cstdint>

#include "Wire.h"

namespace repeater {

/// Addresses every repeater. Only verbs that solicit no reply may use it — six
/// simultaneous answers on a half-duplex multidrop bus is a collision generator.
constexpr int8_t REPEATER_ALL = -2;
constexpr uint8_t REPEATER_COUNT = 6;
constexpr size_t MAC_BYTES = 6;

/// Bumped when the control-plane wire format changes. The host uses it to decide
/// whether a given repeater understands the snapshot and OTA verbs, and degrades
/// per repeater rather than fleet-wide.
constexpr uint16_t CONTROL_PROTO_VERSION = 1;

/// Repeater 1..6 map to -3..-8. Returns 0 for an out-of-range index.
constexpr int8_t repeaterAddress(int8_t index) {
    return (index >= 1 && index <= static_cast<int8_t>(REPEATER_COUNT))
        ? static_cast<int8_t>(-(2 + index))
        : 0;
}

/// The inverse. Returns 0 for anything that is not a repeater unicast address.
constexpr int8_t repeaterIndexFromAddress(int8_t address) {
    return (address <= -3 && address >= -(2 + static_cast<int8_t>(REPEATER_COUNT)))
        ? static_cast<int8_t>(-(address + 2))
        : 0;
}

enum class ControlVerb : uint8_t {
    Unknown = 0,
    Status,
    Relearn,
    ResetCounters,
    Reboot,
    SetIndex,
    SnapshotStart,
    SnapshotRead,
    OtaBegin,
    OtaData,
    OtaMap,
    OtaEnd,
    OtaBoot,
    OtaConfirm,
    OtaAbort,
    /// `v: [side, mode]` -- side 1 or 2, mode 0 normal / 1 inverted / 2 auto. Acknowledged.
    SetPolarity,
};

const char* controlVerbName(ControlVerb verb);

/// True for verbs whose reply would collide if several repeaters answered at once.
/// Those are rejected when sent to `REPEATER_ALL`.
bool controlVerbRepliesToUnicastOnly(ControlVerb verb);

struct ControlRequest {
    /// The frame decoded and looked like a control-plane request at all.
    bool valid = false;
    /// It addresses this repeater, whether by index, MAC, or the broadcast address.
    bool addressedToUs = false;
    /// It used `REPEATER_ALL` rather than naming a single unit.
    bool broadcast = false;
    ControlVerb verb = ControlVerb::Unknown;
    /// Raw MessagePack for the `v` value, pointing into the plane's decode buffer.
    /// Valid until the next `parse()` call.
    const uint8_t* payload = nullptr;
    size_t payloadSize = 0;
};

/// Largest control-plane frame the repeater will build. OTA data chunks travel in
/// the other direction, so replies stay small: the biggest is the received-chunk
/// bitmap, 512 bytes for a 4096-chunk image.
constexpr size_t CONTROL_DECODE_BYTES = 2048;
constexpr size_t CONTROL_REPLY_BYTES = 768;

class ControlPlane {
public:
    ControlPlane();

    void setIdentity(int8_t index, const uint8_t mac[MAC_BYTES]);
    int8_t index() const { return index_; }
    /// 0 when this repeater has no index yet, in which case it can still be
    /// reached by MAC but cannot originate a reply that names a unicast source.
    int8_t address() const { return repeaterAddress(index_); }

    /// Inspect a complete COBS frame from the outer bus.
    ControlRequest parse(const uint8_t* frame, size_t size);

    /// Start a reply. Writes the envelope and the fixed part of the `rr` map; pass
    /// `withPayload` so the map header can be sized before anything is appended.
    /// Append the `v` value through the returned writer when `withPayload` is true.
    wire::MsgpackWriter& beginReply(ControlVerb verb, bool ok, bool withPayload = false);

    /// COBS-frames the reply into `out`. Returns 0 if it overflowed or if this
    /// repeater has no address to answer from.
    size_t finishReply(uint8_t* out, size_t capacity);

private:
    int8_t index_ = 0;
    uint8_t mac_[MAC_BYTES] = {0, 0, 0, 0, 0, 0};
    uint8_t decodeBuffer_[CONTROL_DECODE_BYTES] = {};
    uint8_t replyBuffer_[CONTROL_REPLY_BYTES] = {};
    wire::MsgpackWriter replyWriter_{replyBuffer_, sizeof(replyBuffer_)};
    bool replyOpen_ = false;
};

} // namespace repeater
