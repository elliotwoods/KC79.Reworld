#include "OtaSession.h"

#include <cstring>

#include "Sha256.h"
#include "Wire.h"

namespace repeater {
namespace {

/// Read-back slice for verification. Small enough to sit on the stack of the loop
/// task without crowding anything else.
constexpr size_t VERIFY_SLICE_BYTES = 256;

} // namespace

const char* otaStateName(OtaState state) {
    switch(state) {
    case OtaState::Idle: return "idle";
    case OtaState::Receiving: return "receiving";
    case OtaState::Ready: return "ready";
    case OtaState::Failed: return "failed";
    }
    return "unknown";
}

const char* otaResultName(OtaResult result) {
    switch(result) {
    case OtaResult::Ok: return "ok";
    case OtaResult::NoSession: return "no-session";
    case OtaResult::WrongSession: return "wrong-session";
    case OtaResult::BadRequest: return "bad-request";
    case OtaResult::BadIndex: return "bad-index";
    case OtaResult::BadCrc: return "bad-crc";
    case OtaResult::WriteFailed: return "write-failed";
    case OtaResult::Incomplete: return "incomplete";
    case OtaResult::VerifyFailed: return "verify-failed";
    case OtaResult::CommitFailed: return "commit-failed";
    case OtaResult::EraseFailed: return "erase-failed";
    }
    return "unknown";
}

void OtaSession::reset() {
    state_ = OtaState::Idle;
    session_ = 0;
    imageSize_ = 0;
    chunkBytes_ = 0;
    chunkCount_ = 0;
    receivedChunks_ = 0;
    std::memset(bitmap_, 0, sizeof(bitmap_));
}

bool OtaSession::marked(uint32_t index) const {
    return (bitmap_[index / 8] & (1u << (index % 8))) != 0;
}

void OtaSession::mark(uint32_t index) {
    bitmap_[index / 8] |= static_cast<uint8_t>(1u << (index % 8));
}

OtaResult OtaSession::begin(const OtaBeginRequest& request, uint32_t nowMs) {
    if(request.imageSize == 0 || request.chunkBytes == 0
        || request.chunkBytes > OTA_MAX_CHUNK_BYTES) {
        lastError_ = OtaResult::BadRequest;
        return lastError_;
    }
    const uint32_t chunks = (request.imageSize + request.chunkBytes - 1) / request.chunkBytes;
    if(chunks > OTA_MAX_CHUNKS) {
        lastError_ = OtaResult::BadRequest;
        return lastError_;
    }

    // A second begin while one is open abandons the first explicitly rather than
    // leaking its handle.
    if(state_ == OtaState::Receiving) target_.abortImage();
    reset();

    if(!target_.beginImage(request.imageSize)) {
        state_ = OtaState::Failed;
        lastError_ = OtaResult::EraseFailed;
        return lastError_;
    }

    session_ = request.session;
    imageSize_ = request.imageSize;
    chunkBytes_ = request.chunkBytes;
    chunkCount_ = chunks;
    std::memcpy(expectedSha_, request.sha256, OTA_SHA_BYTES);
    lastActivityMs_ = nowMs;
    state_ = OtaState::Receiving;
    lastError_ = OtaResult::Ok;
    return lastError_;
}

OtaResult OtaSession::writeChunk(uint8_t session, uint32_t index, const uint8_t* data, size_t size,
    uint16_t crc, uint32_t nowMs) {
    // The guard that matters: IDF asserts the partition was erased before a write,
    // and assertions are enabled in this build, so an unguarded stray chunk would
    // panic and reboot the repeater rather than return an error.
    if(state_ != OtaState::Receiving) {
        lastError_ = OtaResult::NoSession;
        return lastError_;
    }
    if(session != session_) {
        lastError_ = OtaResult::WrongSession;
        return lastError_;
    }
    if(index >= chunkCount_ || data == nullptr || size == 0 || size > chunkBytes_) {
        lastError_ = OtaResult::BadIndex;
        return lastError_;
    }
    const uint32_t offset = index * chunkBytes_;
    const uint32_t expectedSize = (index == chunkCount_ - 1) ? (imageSize_ - offset) : chunkBytes_;
    if(size != expectedSize) {
        lastError_ = OtaResult::BadIndex;
        return lastError_;
    }
    if(wire::crc16CcittFalse(data, size) != crc) {
        // Left unmarked, so the gap-repair pass collects it like any other loss.
        lastError_ = OtaResult::BadCrc;
        return lastError_;
    }

    lastActivityMs_ = nowMs;
    if(marked(index)) {
        lastError_ = OtaResult::Ok; // a duplicate from a repeat pass
        return lastError_;
    }
    if(!target_.writeAt(offset, data, size)) {
        lastError_ = OtaResult::WriteFailed;
        return lastError_;
    }
    mark(index);
    receivedChunks_++;
    lastError_ = OtaResult::Ok;
    return lastError_;
}

OtaResult OtaSession::finish(uint32_t nowMs) {
    if(state_ != OtaState::Receiving) {
        lastError_ = OtaResult::NoSession;
        return lastError_;
    }
    lastActivityMs_ = nowMs;
    if(receivedChunks_ != chunkCount_) {
        // Not a failure: the host should read the bitmap and fill the gaps.
        lastError_ = OtaResult::Incomplete;
        return lastError_;
    }

    Sha256 sha;
    uint8_t slice[VERIFY_SLICE_BYTES];
    uint32_t offset = 0;
    while(offset < imageSize_) {
        const uint32_t take = (imageSize_ - offset) < VERIFY_SLICE_BYTES
            ? (imageSize_ - offset)
            : VERIFY_SLICE_BYTES;
        if(!target_.readAt(offset, slice, take)) {
            state_ = OtaState::Failed;
            lastError_ = OtaResult::VerifyFailed;
            return lastError_;
        }
        sha.update(slice, take);
        offset += take;
    }
    uint8_t digest[OTA_SHA_BYTES];
    sha.finish(digest);
    if(std::memcmp(digest, expectedSha_, OTA_SHA_BYTES) != 0) {
        target_.abortImage();
        state_ = OtaState::Failed;
        lastError_ = OtaResult::VerifyFailed;
        return lastError_;
    }

    if(!target_.commit()) {
        state_ = OtaState::Failed;
        lastError_ = OtaResult::CommitFailed;
        return lastError_;
    }
    state_ = OtaState::Ready;
    lastError_ = OtaResult::Ok;
    return lastError_;
}

void OtaSession::abort() {
    if(state_ == OtaState::Receiving) target_.abortImage();
    reset();
}

void OtaSession::service(uint32_t nowMs) {
    if(state_ != OtaState::Receiving) return;
    if(static_cast<uint32_t>(nowMs - lastActivityMs_) < OTA_INACTIVITY_TIMEOUT_MS) return;
    target_.abortImage();
    reset();
    lastError_ = OtaResult::NoSession;
}

} // namespace repeater
