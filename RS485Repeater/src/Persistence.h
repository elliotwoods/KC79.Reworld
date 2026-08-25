#pragma once

#ifdef ARDUINO

#include <cstdint>

namespace repeater {
namespace persistence {

/// Nothing is stored yet.
constexpr int8_t INDEX_UNSET = 0;

/// Opens the NVS namespace. Safe to call once from setup(); `nvs_flash_init()`
/// has already run inside `initArduino()`.
void begin();

/// 0 when unprovisioned, otherwise 1..6. Deliberately not derived from the
/// learned range: a repeater whose branch is dead never learns one, and that is
/// exactly when it most needs to be addressable.
int8_t repeaterIndex();

/// Accepts 0 (clear) or 1..6. Returns false and stores nothing otherwise.
bool setRepeaterIndex(int8_t index);

/// The nine-ID block start persisted at the last learn, or 0 if none. Restoring
/// it stops a cold boot from failing open and flooding the branch with all 54
/// unicasts until an inner reply happens to arrive.
uint8_t learnedRangeStart();
void setLearnedRangeStart(uint8_t rangeStart);

/// Total boots, for forensics.
uint32_t bootCount();

/// Boots since the application last declared itself healthy. Used by the OTA
/// rollback decision to recognise an image that will not stay up.
uint32_t unhealthyBoots();

/// Call once at startup: bumps the boot count and the unhealthy-boot counter.
void noteBootAttempt();

/// Call once the application has demonstrably run: clears the unhealthy-boot
/// counter. A no-op when it is already zero, so it costs no flash write.
void noteHealthy();

} // namespace persistence
} // namespace repeater

#endif
