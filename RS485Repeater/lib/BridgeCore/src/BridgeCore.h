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

constexpr size_t MAX_FRAME_BYTES = 8192;
constexpr size_t FRAME_QUEUE_DEPTH = 4;

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
};

struct FrameView {
    Side source = Side::None;
    const uint8_t* data = nullptr;
    size_t size = 0;
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

    void relearn();
    void resetStats();

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

    struct EnvelopeInfo {
        bool valid = false;
        bool parseError = false;
        int64_t target = 0;
        int64_t source = 0;
        bool isKeyframe = false;
        uint64_t keyframeStart = 0;
        uint64_t keyframeCount = 0;
    };

    Accumulator& accumulator(Side source);
    const Accumulator& accumulator(Side source) const;
    FrameQueue& queue(Side source);
    const FrameQueue& queue(Side source) const;
    DirectionStats& direction(Side source);

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
    std::array<uint8_t, MAX_FRAME_BYTES> decodeBuffer_{};
    RoutingMode routingMode_ = RoutingMode::Transparent;
    uint8_t localRangeStart_ = 0;
    Side lastDequeued_ = Side::Two;
    RouterStats stats_;
};

const char* routingModeName(RoutingMode mode);

} // namespace repeater
