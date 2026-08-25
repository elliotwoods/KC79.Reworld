#pragma once

/// Branch-local position sweeps.
///
/// The host broadcasts `snap-start`, and all six repeaters poll their own nine
/// Portals at the same time — the branches are electrically isolated, so those
/// sweeps genuinely run in parallel. The host then reads each repeater in turn.
/// That keeps the host the single arbiter of the shared bus: no distributed clock,
/// no time slots, and nothing that breaks when a store-and-forward write happens to
/// block a repeater's loop for tens of milliseconds.
///
/// The read relays the stored Portal replies **verbatim** rather than repacking
/// them. That costs about a hundred extra bytes per branch and buys a great deal:
/// the host needs no new parser, and PortalFW's own `[..., seq, crc16]` trailer
/// survives end to end instead of being discarded and replaced with nothing.

#include <cstddef>
#include <cstdint>

#include "BridgeCore.h"

namespace repeater {

constexpr uint8_t SNAPSHOT_BRANCH_SIZE = 9;

/// A `{"p": [...]}` reply is around 36 bytes framed, including the seq/CRC trailer
/// PortalFW appends. This leaves generous room for a longer one.
constexpr size_t SNAPSHOT_MAX_REPLY_BYTES = 96;

constexpr uint32_t SNAPSHOT_DEFAULT_COLLECT_MS = 60;
constexpr uint32_t SNAPSHOT_MIN_POLL_TIMEOUT_MS = 6;
constexpr uint32_t SNAPSHOT_MAX_POLL_TIMEOUT_MS = 40;

class SnapshotEngine : public InnerReplyConsumer {
public:
    /// Starts a sweep of `rangeStart .. rangeStart + 8`. A start while one is
    /// already running replaces it; the host is the only thing that issues these.
    void begin(uint8_t rangeStart, uint32_t collectMs, uint32_t nowMs);

    /// Builds the next branch poll when one is due, returning its framed size.
    /// Returns 0 when the sweep is idle, finished, or still waiting for a reply.
    size_t nextPoll(uint32_t nowMs, uint8_t* out, size_t capacity);

    /// Claims the reply to the poll currently outstanding. Anything else — a log
    /// message, a late reply, a duplicate — is left to be relayed upstream, so the
    /// repeater does not go blind to branch faults for the length of a sweep.
    bool consumeInnerReply(const InnerFrameInfo& info, const uint8_t* frame, size_t size) override;

    /// Call regularly so a sweep whose last poll went unanswered still ends.
    void service(uint32_t nowMs);

    bool collecting() const { return collecting_; }
    uint8_t rangeStart() const { return rangeStart_; }
    uint8_t storedCount() const { return storedCount_; }

    /// Bit i set means `rangeStart + i` answered this sweep.
    uint16_t receivedMask() const { return receivedMask_; }

    /// The stored replies, in the order they were collected.
    const uint8_t* storedFrame(uint8_t slot, size_t& size) const;

    /// How long the last completed sweep took, for the host's timing picture.
    uint32_t lastSweepMs() const { return lastSweepMs_; }

private:
    void advance();

    bool collecting_ = false;
    bool awaitingReply_ = false;
    bool finishPending_ = false;
    uint8_t rangeStart_ = 0;
    uint8_t cursor_ = 0;
    uint8_t storedCount_ = 0;
    uint16_t receivedMask_ = 0;
    uint32_t startedMs_ = 0;
    uint32_t collectMs_ = SNAPSHOT_DEFAULT_COLLECT_MS;
    uint32_t pollTimeoutMs_ = SNAPSHOT_MIN_POLL_TIMEOUT_MS;
    uint32_t pollSentMs_ = 0;
    uint32_t lastSweepMs_ = 0;

    uint8_t frames_[SNAPSHOT_BRANCH_SIZE][SNAPSHOT_MAX_REPLY_BYTES] = {};
    size_t frameSizes_[SNAPSHOT_BRANCH_SIZE] = {};
};

} // namespace repeater
