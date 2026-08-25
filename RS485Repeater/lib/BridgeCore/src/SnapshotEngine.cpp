#include "SnapshotEngine.h"

#include <cstring>

#include "Wire.h"

namespace repeater {
namespace {

uint32_t clampU32(uint32_t value, uint32_t low, uint32_t high) {
    if(value < low) return low;
    if(value > high) return high;
    return value;
}

} // namespace

void SnapshotEngine::begin(uint8_t rangeStart, uint32_t collectMs, uint32_t nowMs) {
    if(rangeStart == 0) return;
    rangeStart_ = rangeStart;
    collectMs_ = collectMs == 0 ? SNAPSHOT_DEFAULT_COLLECT_MS : collectMs;
    // Derived rather than configured separately, so a caller cannot ask for a
    // per-poll timeout that could not possibly fit nine polls in the window.
    pollTimeoutMs_ = clampU32(collectMs_ / SNAPSHOT_BRANCH_SIZE,
        SNAPSHOT_MIN_POLL_TIMEOUT_MS, SNAPSHOT_MAX_POLL_TIMEOUT_MS);
    cursor_ = 0;
    storedCount_ = 0;
    receivedMask_ = 0;
    awaitingReply_ = false;
    startedMs_ = nowMs;
    pollSentMs_ = nowMs;
    collecting_ = true;
    finishPending_ = false;
    std::memset(frameSizes_, 0, sizeof(frameSizes_));
}

void SnapshotEngine::advance() {
    awaitingReply_ = false;
    cursor_++;
    if(cursor_ >= SNAPSHOT_BRANCH_SIZE) {
        collecting_ = false;
        // Timed on the next service tick, which has a real clock. A reply arrives
        // from inside frame ingest, which does not.
        finishPending_ = true;
    }
}

size_t SnapshotEngine::nextPoll(uint32_t nowMs, uint8_t* out, size_t capacity) {
    if(!collecting_ || out == nullptr) return 0;

    if(static_cast<uint32_t>(nowMs - startedMs_) >= collectMs_) {
        // The window closed. Whatever did not answer is reported as missing rather
        // than holding the branch any longer.
        collecting_ = false;
        awaitingReply_ = false;
        finishPending_ = false;
        lastSweepMs_ = static_cast<uint32_t>(nowMs - startedMs_);
        return 0;
    }

    if(awaitingReply_) {
        if(static_cast<uint32_t>(nowMs - pollSentMs_) < pollTimeoutMs_) return 0;
        advance();
        if(!collecting_) {
            finishPending_ = false;
            lastSweepMs_ = static_cast<uint32_t>(nowMs - startedMs_);
            return 0;
        }
    }

    // `[id, 0, {"p": nil}]`, the same non-motion position poll the host uses.
    uint8_t body[16];
    wire::MsgpackWriter writer(body, sizeof(body));
    writer.arrayHeader(3);
    writer.integer(rangeStart_ + cursor_);
    writer.integer(0);
    writer.mapHeader(1);
    writer.key("p");
    writer.nil();
    if(!writer.ok()) return 0;

    const size_t framed = wire::cobsEncodeFrame(writer.data(), writer.size(), out, capacity);
    if(framed == 0) return 0;

    awaitingReply_ = true;
    pollSentMs_ = nowMs;
    return framed;
}

bool SnapshotEngine::consumeInnerReply(const InnerFrameInfo& info, const uint8_t* frame, size_t size) {
    if(!collecting_ || !awaitingReply_) return false;
    if(info.target != 0 || !info.isPositionReply) return false;
    // Only the poll actually outstanding is claimed. A duplicate, a late reply from
    // an earlier sweep, or anything from another Portal is relayed as usual.
    if(info.source != static_cast<int64_t>(rangeStart_ + cursor_)) return false;
    if(size == 0 || size > SNAPSHOT_MAX_REPLY_BYTES) return false;

    std::memcpy(frames_[storedCount_], frame, size);
    frameSizes_[storedCount_] = size;
    storedCount_++;
    receivedMask_ |= static_cast<uint16_t>(1u << cursor_);
    advance();
    return true;
}

void SnapshotEngine::service(uint32_t nowMs) {
    if(!collecting_) {
        if(finishPending_) {
            finishPending_ = false;
            lastSweepMs_ = static_cast<uint32_t>(nowMs - startedMs_);
        }
        return;
    }
    if(static_cast<uint32_t>(nowMs - startedMs_) < collectMs_) return;
    collecting_ = false;
    awaitingReply_ = false;
    finishPending_ = false;
    lastSweepMs_ = static_cast<uint32_t>(nowMs - startedMs_);
}

const uint8_t* SnapshotEngine::storedFrame(uint8_t slot, size_t& size) const {
    if(slot >= storedCount_) {
        size = 0;
        return nullptr;
    }
    size = frameSizes_[slot];
    return frames_[slot];
}

} // namespace repeater
