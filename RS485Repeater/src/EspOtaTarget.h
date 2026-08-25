#pragma once

#ifdef ARDUINO

#include <esp_ota_ops.h>

#include "OtaSession.h"

namespace repeater {

/// `OtaSession`'s flash side, on top of `esp_ota_*`.
///
/// Writes go through `esp_ota_write_with_offset`, which is what allows chunks to
/// arrive out of order and lets a repair pass fill gaps. That requires the slot to
/// have been erased up front, so `beginImage` passes the exact image size rather
/// than `OTA_SIZE_UNKNOWN`, and never uses `OTA_WITH_SEQUENTIAL_WRITES`.
class EspOtaTarget : public OtaTarget {
public:
    bool beginImage(uint32_t imageSize) override;
    bool writeAt(uint32_t offset, const uint8_t* data, size_t size) override;
    bool readAt(uint32_t offset, uint8_t* data, size_t size) override;
    bool commit() override;
    void abortImage() override;

    /// Label of the partition currently running, for diagnostics.
    static const char* runningLabel();

    /// True while the running image still has to prove itself. Arduino would
    /// normally resolve this inside `initArduino()`; the sketch overrides
    /// `verifyRollbackLater()` so the decision belongs to the application.
    static bool pendingVerify();

    /// Cancels the pending rollback and keeps the running image.
    static bool markValid();

    /// Reverts to the previous slot and reboots. Returns false only if it could
    /// not be started — notably when the other slot holds no valid firmware,
    /// which is the state left behind by an aborted update.
    static bool rollBackAndReboot();

private:
    esp_ota_handle_t handle_ = 0;
    const esp_partition_t* partition_ = nullptr;
    bool open_ = false;
};

} // namespace repeater

#endif
