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

/// Whether this panel knows which nine Portal IDs are its own.
///
/// It no longer decides any forwarding -- in a chain everything is relayed downstream
/// regardless -- but the snapshot sweep has to know which nine boards to poll, and the
/// block cannot be inferred from traffic any more. A panel sees replies from every panel
/// below it transiting its own bus, so "the first reply tells me my block" learns
/// whichever panel answered first, and "a reply from another block is a conflict" fires
/// on ordinary transit. The block comes from the provisioned index instead.
enum class BlockState : uint8_t {
    Unknown = 0,
    Assigned = 1,
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

/// How long a part-received frame may sit before it is abandoned as truncated.
///
/// It lives here rather than beside the pin numbers so the firmware and the tests
/// cannot hold different opinions about it; a value that only the Arduino build sees
/// is one the native suite can never catch a regression in.
///
/// It was 2 ms -- 23 byte-times at 115200, shorter than the frames it was protecting.
/// Bisected on the bench 2026-08-26: a 159-byte frame relayed, a 160-byte one silently
/// discarded, because a buffered host writer delivers a frame in 64-byte USB packets and
/// the gap between them exceeded the timer once a frame spanned more than two. It cost a
/// whole ESP32 firmware transfer, and it sat directly under the bootloader control plane,
/// whose frames are ~149 bytes.
///
/// 20 ms is still well below the 178 ms a full 2048-byte frame occupies, and COBS
/// delimiters -- not this timer -- are what separate frames, so a stream that genuinely
/// stopped is still abandoned long before the next one could be joined to it.
constexpr uint32_t FRAME_IDLE_TIMEOUT_US = 20000;

struct DirectionStats {
    uint64_t rxBytes = 0;
    uint64_t receivedFrames = 0;
    uint64_t forwardedBytes = 0;
    uint64_t forwardedFrames = 0;
    uint64_t incompleteFrames = 0;
    uint64_t oversizedFrames = 0;
    /// Delimiters that closed nothing -- an empty COBS packet. Absorbed, not relayed.
    /// Counted because it is the only direct measure of turn-around glitching on this side.
    uint64_t emptyFrames = 0;
    uint64_t queueDrops = 0;
    uint32_t queueHighWater = 0;
};

struct RouterStats {
    DirectionStats oneToTwo;
    DirectionStats twoToOne;
    uint64_t parseErrors = 0;
    /// Host-addressed frames that were not repeater-plane traffic: a Portal reply
    /// arriving from the wrong direction. Never relayed into a branch.
    uint64_t filteredHostFrames = 0;
    uint64_t txErrors = 0;
    /// Repeater-plane frames passed down the chain because they belong to a panel
    /// below this one.
    uint64_t relayedControlFrames = 0;
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

/// What a repeater-plane frame arriving on the upstream side should have done with it.
///
/// Panels are wired as a chain, not a star: this repeater's downstream bus is also the
/// uplink for the next panel, so a control frame addressed to a panel below this one can
/// only get there by being relayed. Dropping it -- which is what a star topology could
/// afford to do, because every repeater heard the host directly -- makes every panel
/// past the first permanently unreachable for status, provisioning and OTA.
enum class ControlDisposition : uint8_t {
    /// Not repeater-plane traffic. A host-addressed frame that is not a request is a
    /// reply that has come the wrong way, and must not enter a branch.
    NotControl = 0,
    /// Addressed to this repeater. Acted on, and it stops here.
    Consumed,
    /// Broadcast to every repeater. Acted on here, and every panel below needs it too.
    ConsumedAndRelay,
    /// Addressed to a different repeater. Not ours to act on; relay it unchanged.
    Relay,
};

/// Lets the control plane claim, or route, host-addressed frames arriving upstream.
class ControlFrameConsumer {
public:
    virtual ~ControlFrameConsumer() = default;

    /// Classify a repeater-plane frame, acting on it if it is ours.
    virtual ControlDisposition consumeControlFrame(const uint8_t* frame, size_t size) = 0;
};

/// Bounded, store-and-forward router for zero-delimited COBS frames.
///
/// Side One faces the host -- directly for the first panel, and through the panel above
/// for every other. Side Two carries this panel's Portals *and* the next panel's uplink.
///
/// Because of that, upstream-to-downstream is deliberately transparent: anything this
/// panel does not consume is relayed, because the traffic for every panel below has to
/// cross this one. There is no address filtering to be had -- the frames a filter would
/// drop are exactly the frames the rest of the chain is waiting for.
class FrameRouter {
public:
    explicit FrameRouter(uint32_t idleTimeoutUs = FRAME_IDLE_TIMEOUT_US);

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

    /// Forget the local block. The snapshot sweep stops until an index is provisioned.
    void clearLocalBlock();
    void resetStats();

    /// How long an unterminated frame is allowed to stall before it is abandoned.
    ///
    /// This is a store-and-forward bridge, so the whole frame has to arrive before any of
    /// it is relayed, and a gap longer than this discards what has accumulated. Two
    /// milliseconds is ample for a frame a host writes in one call, and far too tight for
    /// one a Portal *generates* as it transmits: a full status reply is built through a
    /// 256-byte COBS buffer while the application keeps running, so it reaches the wire in
    /// bursts with real gaps between them, and the bridge chops it into fragments that
    /// decode as nothing.
    void setIdleTimeoutUs(uint32_t microseconds) { idleTimeoutUs_ = microseconds; }
    uint32_t idleTimeoutUs() const { return idleTimeoutUs_; }

    /// Declare which nine Portal IDs belong to this panel, from its provisioned index.
    /// Rejects anything that is not a legal block start.
    void setLocalBlock(uint8_t rangeStart);

    BlockState blockState() const { return blockState_; }
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

    uint32_t idleTimeoutUs_;
    Accumulator sideOneAccumulator_;
    Accumulator sideTwoAccumulator_;
    FrameQueue oneToTwoQueue_;
    FrameQueue twoToOneQueue_;
    OriginateQueue originateToOneQueue_;
    OriginateQueue originateToTwoQueue_;
    std::array<uint8_t, MAX_FRAME_BYTES> decodeBuffer_{};
    BlockState blockState_ = BlockState::Unknown;
    uint8_t localRangeStart_ = 0;
    Side lastDequeued_ = Side::Two;
    Side lastOriginated_ = Side::Two;
    InnerReplyConsumer* innerReplyConsumer_ = nullptr;
    ControlFrameConsumer* controlFrameConsumer_ = nullptr;
    bool forwardingPaused_ = false;
    RouterStats stats_;
};

const char* blockStateName(BlockState state);

} // namespace repeater
