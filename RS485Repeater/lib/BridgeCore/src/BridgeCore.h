#pragma once

#include <array>
#include <cstddef>
#include <cstdint>

namespace repeater {

enum class Side : uint8_t {
    None = 0,
    One = 1, // shared outer / host bus
    Two = 2, // local Portal bus
};

enum class RoutingMode : uint8_t {
    Transparent = 0,
    Filtered = 1,
    Conflict = 2,
};

/// Largest relayed frame. Deliberately far below the previous 8192: a frame is
/// written to the destination UART with the driver enabled and the loop blocked,
/// and at 115200 baud 8192 bytes is 711 ms of stalled bridge against 178 ms here.
///
/// The headroom is measured, not estimated. `router-proto`'s
/// `the_largest_configurable_keyframe_fits_the_repeater_frame_limit` encodes
/// worst-case keyframes with the real host encoder: the nine-entry batch V3 uses
/// is 225 framed bytes, and the largest anyone could configure -- 54 entries with
/// full-range positions and velocities -- is 1172. That test fails if a future
/// change would push a legitimate frame past this limit, which would otherwise
/// show up only as frames being silently discarded in the field.
constexpr size_t MAX_FRAME_BYTES = 2048;

/// Depth of each relay queue. Sixteen slots absorb the roughly twelve keyframes the
/// host can emit during a branch poll sweep at the V3 5 ms broadcast gap. Total queue
/// RAM is unchanged from the previous 4 x 8192 arrangement.
constexpr size_t FRAME_QUEUE_DEPTH = 16;

/// Locally originated frames (branch polls, control-plane and OTA replies) are small
/// and low-volume, so they get their own much smaller queues rather than paying the
/// relay frame size.
constexpr size_t MAX_ORIGINATED_FRAME_BYTES = 512;
constexpr size_t ORIGINATE_QUEUE_DEPTH = 4;

struct DirectionStats {
    uint64_t rxBytes = 0;
    uint64_t receivedFrames = 0;
    uint64_t forwardedBytes = 0;
    uint64_t forwardedFrames = 0;
    uint64_t incompleteFrames = 0;
    uint64_t oversizedFrames = 0;
    uint64_t queueDrops = 0;
    uint32_t queueHighWater = 0;
};

struct RouterStats {
    DirectionStats oneToTwo;
    DirectionStats twoToOne;
    uint64_t parseErrors = 0;
    uint64_t filteredUnicasts = 0;
    uint64_t filteredKeyframes = 0;
    uint64_t filteredHostFrames = 0;
    uint64_t topologyConflicts = 0;
    uint64_t txErrors = 0;
    /// Side-2 frames a local consumer claimed, so they were not relayed upstream.
    uint64_t consumedInnerFrames = 0;
    uint64_t originatedFrames = 0;
    uint64_t originateDrops = 0;
    /// Outer-bus frames the control plane recognised as addressed to a repeater.
    uint64_t controlFrames = 0;
    /// Frames discarded because relaying was paused for maintenance.
    uint64_t pausedDrops = 0;
};

struct FrameView {
    /// `Side::None` for a locally originated frame.
    Side source = Side::None;
    /// The side this frame must be transmitted on.
    Side destination = Side::None;
    const uint8_t* data = nullptr;
    size_t size = 0;
};

/// What `inspectEnvelope` learned about a frame arriving from the local branch.
struct InnerFrameInfo {
    int64_t target = 0;
    int64_t source = 0;
    bool isPositionReply = false;
};

/// Lets a local subsystem claim branch replies it solicited itself, instead of
/// letting them be relayed upstream. Without this every poll a snapshot sweep
/// issues would reach the host as well as the aggregate.
class InnerReplyConsumer {
public:
    virtual ~InnerReplyConsumer() = default;

    /// Return true if this frame was consumed locally and must not be relayed.
    virtual bool consumeInnerReply(const InnerFrameInfo& info, const uint8_t* frame, size_t size) = 0;
};

/// Lets the control plane claim host-addressed frames arriving on the outer bus.
///
/// Repeater-plane frames are addressed with envelope target 0 precisely because
/// that is the one class this router has always refused to forward to the branch,
/// in every routing mode. A repeater running older firmware therefore ignores
/// control traffic rather than relaying it to nine Portals. Whether or not the
/// consumer claims a frame, it is still never forwarded.
class ControlFrameConsumer {
public:
    virtual ~ControlFrameConsumer() = default;

    /// Return true if this frame was addressed to the repeater plane.
    virtual bool consumeControlFrame(const uint8_t* frame, size_t size) = 0;
};

/// Bounded, store-and-forward router for zero-delimited COBS frames.
///
/// Side One is the shared host bus and Side Two is the local Portal branch.
/// Unknown traffic is deliberately fail-open. Once a valid reply from a local
/// Portal identifies a nine-ID block, only understood non-local unicasts and
/// keyframes are suppressed on the outer-to-inner path.
class FrameRouter {
public:
    explicit FrameRouter(uint32_t idleTimeoutUs = 2000);

    void ingest(Side source, const uint8_t* bytes, size_t count, uint32_t nowUs);
    void expireIncomplete(uint32_t nowUs);

    bool nextFrame(FrameView& view);
    void completeTransmission(Side source, bool success = true);

    /// Queue a locally generated frame for transmission on `destination`. The bytes
    /// must already be a complete COBS frame including its terminating zero.
    /// Returns false if the frame is too large or the queue is full.
    bool originate(Side destination, const uint8_t* data, size_t size);
    void completeOriginated(Side destination, bool success = true);
    size_t originateDepth(Side destination) const;

    void setInnerReplyConsumer(InnerReplyConsumer* consumer) { innerReplyConsumer_ = consumer; }
    void setControlFrameConsumer(ControlFrameConsumer* consumer) { controlFrameConsumer_ = consumer; }

    /// Maintenance mode. Relaying stops in both directions and the traffic is
    /// discarded rather than queued, so an update does not end with a burst of
    /// stale keyframes. The control plane keeps working throughout, which is what
    /// lets a paused repeater still be told to resume.
    void setForwardingPaused(bool paused) { forwardingPaused_ = paused; }
    bool forwardingPaused() const { return forwardingPaused_; }

    void relearn();
    void resetStats();

    /// Adopt a previously learned block without waiting for a branch reply. Used to
    /// restore the range persisted at the last shutdown, so a cold boot does not
    /// fail open and flood the branch with all 54 unicasts.
    void restoreLearnedRange(uint8_t rangeStart);

    RoutingMode routingMode() const { return routingMode_; }
    uint8_t localRangeStart() const { return localRangeStart_; }
    uint8_t localRangeEnd() const {
        return localRangeStart_ == 0 ? 0 : static_cast<uint8_t>(localRangeStart_ + 8);
    }
    size_t queueDepth(Side source) const;
    const RouterStats& stats() const { return stats_; }

private:
    struct Accumulator {
        std::array<uint8_t, MAX_FRAME_BYTES> data{};
        size_t size = 0;
        uint32_t lastByteUs = 0;
        bool discardingOversize = false;
    };

    struct StoredFrame {
        std::array<uint8_t, MAX_FRAME_BYTES> data{};
        size_t size = 0;
    };

    struct FrameQueue {
        std::array<StoredFrame, FRAME_QUEUE_DEPTH> frames{};
        size_t head = 0;
        size_t count = 0;
    };

    struct OriginatedFrame {
        std::array<uint8_t, MAX_ORIGINATED_FRAME_BYTES> data{};
        size_t size = 0;
    };

    struct OriginateQueue {
        std::array<OriginatedFrame, ORIGINATE_QUEUE_DEPTH> frames{};
        size_t head = 0;
        size_t count = 0;
    };

    struct EnvelopeInfo {
        bool valid = false;
        bool parseError = false;
        int64_t target = 0;
        int64_t source = 0;
        bool isKeyframe = false;
        bool isPositionReply = false;
        uint64_t keyframeStart = 0;
        uint64_t keyframeCount = 0;
    };

    Accumulator& accumulator(Side source);
    const Accumulator& accumulator(Side source) const;
    FrameQueue& queue(Side source);
    const FrameQueue& queue(Side source) const;
    OriginateQueue& originateQueue(Side destination);
    const OriginateQueue& originateQueue(Side destination) const;
    DirectionStats& direction(Side source);
    bool destinationQuiet(Side destination) const;

    void ingestByte(Side source, uint8_t value, uint32_t nowUs);
    void finishFrame(Side source);
    bool enqueue(Side source, const uint8_t* data, size_t size);
    bool shouldForward(Side source, const uint8_t* data, size_t size);
    EnvelopeInfo inspectEnvelope(const uint8_t* data, size_t size);
    void observeLocalReply(const EnvelopeInfo& envelope);

    uint32_t idleTimeoutUs_;
    Accumulator sideOneAccumulator_;
    Accumulator sideTwoAccumulator_;
    FrameQueue oneToTwoQueue_;
    FrameQueue twoToOneQueue_;
    OriginateQueue originateToOneQueue_;
    OriginateQueue originateToTwoQueue_;
    std::array<uint8_t, MAX_FRAME_BYTES> decodeBuffer_{};
    RoutingMode routingMode_ = RoutingMode::Transparent;
    uint8_t localRangeStart_ = 0;
    Side lastDequeued_ = Side::Two;
    Side lastOriginated_ = Side::Two;
    InnerReplyConsumer* innerReplyConsumer_ = nullptr;
    ControlFrameConsumer* controlFrameConsumer_ = nullptr;
    bool forwardingPaused_ = false;
    RouterStats stats_;
};

const char* routingModeName(RoutingMode mode);

} // namespace repeater
