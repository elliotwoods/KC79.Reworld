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

/// Per-side UART polarity. `side` is 1 or 2. The mode is a `PolarityMode` value
/// (0 normal, 1 inverted, 2 auto); `fallback` is what an unset entry reads as.
/// The inverted flag is the last polarity the wire proved, so a reboot starts
/// where the previous run finished rather than hunting again.
uint8_t polarityMode(uint8_t side, uint8_t fallback);
void setPolarityMode(uint8_t side, uint8_t mode);
bool polarityInverted(uint8_t side, bool fallback);
void setPolarityInverted(uint8_t side, bool inverted);

/// Whether the UART peripheral times driver-enable itself (true) or the loop
/// toggles the GPIO around each frame (false). Read at boot only.
bool hardwareDriverEnable(bool fallback);
void setHardwareDriverEnable(bool enabled);

} // namespace persistence
} // namespace repeater

#endif
