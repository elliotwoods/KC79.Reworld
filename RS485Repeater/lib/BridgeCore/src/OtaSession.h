#pragma once

/// In-band firmware update for the repeater itself.
///
/// The transfer is deliberately unlike the Portal bootloader's, which requires
/// strictly sequential offsets and has no way to say it lost a frame. Here every
/// chunk carries its own index, so chunks may arrive in any order, and the
/// repeater keeps a bitmap of what it actually received. The host reads that
/// bitmap back and resends only the gaps. A whole-image SHA-256 is checked by
/// reading the slot back before it is ever made bootable.
///
/// Flash access is behind `OtaTarget` so the whole state machine, including image
/// verification, runs in the native test environment.

#include <cstddef>
#include <cstdint>

namespace repeater {

/// 4 MB at the smallest sensible chunk size. The bitmap costs one byte per eight.
constexpr uint32_t OTA_MAX_CHUNKS = 4096;
constexpr size_t OTA_BITMAP_BYTES = OTA_MAX_CHUNKS / 8;
constexpr uint32_t OTA_MAX_CHUNK_BYTES = 1024;
constexpr size_t OTA_SHA_BYTES = 32;

/// An abandoned session must not leave the bridge paused indefinitely; for six
/// units in a ceiling that is a ladder rather than an inconvenience.
constexpr uint32_t OTA_INACTIVITY_TIMEOUT_MS = 30000;

enum class OtaState : uint8_t {
    Idle = 0,
    /// Slot erased, accepting chunks.
    Receiving,
    /// Image verified and marked bootable; waiting for a reboot.
    Ready,
    Failed,
};

enum class OtaResult : uint8_t {
    Ok = 0,
    NoSession,
    WrongSession,
    BadRequest,
    BadIndex,
    BadCrc,
    WriteFailed,
    Incomplete,
    VerifyFailed,
    CommitFailed,
    EraseFailed,
};

const char* otaStateName(OtaState state);
const char* otaResultName(OtaResult result);

struct OtaBeginRequest {
    uint32_t imageSize = 0;
    uint32_t chunkBytes = 0;
    /// Distinguishes one transfer from the next. Carried on every chunk, not just
    /// here: without it a repeater that missed the end of transfer A would write
    /// transfer B's chunks into A's half-populated slot.
    uint8_t session = 0;
    uint8_t sha256[OTA_SHA_BYTES] = {};
};

/// The flash side of an update. The production implementation wraps `esp_ota_*`.
class OtaTarget {
public:
    virtual ~OtaTarget() = default;

    /// Prepare the inactive slot for exactly `imageSize` bytes. This erases, which
    /// on real hardware disables the cache for long enough to drop inbound UART
    /// bytes — which is why the caller answers only once this has returned.
    virtual bool beginImage(uint32_t imageSize) = 0;
    virtual bool writeAt(uint32_t offset, const uint8_t* data, size_t size) = 0;
    virtual bool readAt(uint32_t offset, uint8_t* data, size_t size) = 0;
    /// Finalise and mark the slot bootable.
    virtual bool commit() = 0;
    virtual void abortImage() = 0;
};

class OtaSession {
public:
    explicit OtaSession(OtaTarget& target) : target_(target) { }

    OtaResult begin(const OtaBeginRequest& request, uint32_t nowMs);

    /// `crc` is CRC-16/CCITT-FALSE over `data`. A chunk that fails it is discarded
    /// and left unmarked, so the gap repair pass picks it up like any other loss.
    OtaResult writeChunk(uint8_t session, uint32_t index, const uint8_t* data, size_t size,
        uint16_t crc, uint32_t nowMs);

    /// Reads the slot back, checks the whole-image SHA-256, and commits.
    OtaResult finish(uint32_t nowMs);

    void abort();

    /// Call regularly. Aborts a session that has gone quiet.
    void service(uint32_t nowMs);

    OtaState state() const { return state_; }
    OtaResult lastError() const { return lastError_; }
    uint8_t session() const { return session_; }
    uint32_t chunkCount() const { return chunkCount_; }
    uint32_t receivedChunks() const { return receivedChunks_; }
    uint32_t imageSize() const { return imageSize_; }

    /// Received-chunk bitmap, least significant bit first within each byte. Sent to
    /// the host verbatim rather than as run-lengths: it is a fixed 78 bytes for a
    /// 617-chunk image, where alternating gaps would make run-lengths 1.6 kB.
    const uint8_t* bitmap() const { return bitmap_; }
    size_t bitmapBytes() const { return (chunkCount_ + 7) / 8; }

    bool busy() const { return state_ == OtaState::Receiving; }

private:
    void reset();
    bool marked(uint32_t index) const;
    void mark(uint32_t index);

    OtaTarget& target_;
    OtaState state_ = OtaState::Idle;
    OtaResult lastError_ = OtaResult::Ok;
    uint8_t session_ = 0;
    uint32_t imageSize_ = 0;
    uint32_t chunkBytes_ = 0;
    uint32_t chunkCount_ = 0;
    uint32_t receivedChunks_ = 0;
    uint32_t lastActivityMs_ = 0;
    uint8_t expectedSha_[OTA_SHA_BYTES] = {};
    uint8_t bitmap_[OTA_BITMAP_BYTES] = {};
};

} // namespace repeater
