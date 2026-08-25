#ifdef ARDUINO

#include "EspOtaTarget.h"

namespace repeater {

bool EspOtaTarget::beginImage(uint32_t imageSize) {
    abortImage();
    partition_ = esp_ota_get_next_update_partition(nullptr);
    if(partition_ == nullptr) return false;
    if(imageSize > partition_->size) return false;

    // The exact size, so the partition is erased up front and out-of-order writes
    // are legal afterwards. This erase is also why the caller must not answer the
    // host until it returns: the UART ISR lives in flash and cannot run while the
    // cache is disabled, so inbound bytes are lost for the duration.
    if(esp_ota_begin(partition_, imageSize, &handle_) != ESP_OK) {
        partition_ = nullptr;
        return false;
    }
    open_ = true;
    return true;
}

bool EspOtaTarget::writeAt(uint32_t offset, const uint8_t* data, size_t size) {
    if(!open_) return false;
    return esp_ota_write_with_offset(handle_, data, size, offset) == ESP_OK;
}

bool EspOtaTarget::readAt(uint32_t offset, uint8_t* data, size_t size) {
    if(partition_ == nullptr) return false;
    return esp_partition_read(partition_, offset, data, size) == ESP_OK;
}

bool EspOtaTarget::commit() {
    if(!open_ || partition_ == nullptr) return false;
    open_ = false;
    if(esp_ota_end(handle_) != ESP_OK) {
        partition_ = nullptr;
        return false;
    }
    const bool ok = esp_ota_set_boot_partition(partition_) == ESP_OK;
    partition_ = nullptr;
    return ok;
}

void EspOtaTarget::abortImage() {
    if(open_) {
        esp_ota_abort(handle_);
        open_ = false;
    }
    partition_ = nullptr;
}

const char* EspOtaTarget::runningLabel() {
    const esp_partition_t* running = esp_ota_get_running_partition();
    return running != nullptr ? running->label : "unknown";
}

bool EspOtaTarget::pendingVerify() {
    const esp_partition_t* running = esp_ota_get_running_partition();
    if(running == nullptr) return false;
    esp_ota_img_states_t state;
    if(esp_ota_get_state_partition(running, &state) != ESP_OK) return false;
    return state == ESP_OTA_IMG_PENDING_VERIFY;
}

bool EspOtaTarget::markValid() {
    return esp_ota_mark_app_valid_cancel_rollback() == ESP_OK;
}

bool EspOtaTarget::rollBackAndReboot() {
    // Fails with ESP_ERR_OTA_ROLLBACK_FAILED when the other slot holds nothing
    // bootable, which is exactly the state an aborted update leaves behind. The
    // caller has to keep running rather than assume this took effect.
    return esp_ota_mark_app_invalid_rollback_and_reboot() == ESP_OK;
}

} // namespace repeater

#endif
