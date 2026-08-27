#ifdef ARDUINO

#include "Persistence.h"

#include <Preferences.h>

namespace repeater {
namespace persistence {

namespace {

// NVS keys are capped at 15 characters.
constexpr const char* NAMESPACE = "repeater";
constexpr const char* KEY_INDEX = "idx";
constexpr const char* KEY_RANGE = "range";
constexpr const char* KEY_BOOTS = "boots";
constexpr const char* KEY_UNHEALTHY = "unhealthy";
constexpr const char* KEY_POLARITY_MODE[2] = {"pol1", "pol2"};
constexpr const char* KEY_POLARITY_INVERTED[2] = {"inv1", "inv2"};
constexpr const char* KEY_HARDWARE_DE = "dehw";

Preferences store;
bool opened = false;

const char* sideKey(const char* const keys[2], uint8_t side) {
    return keys[side == 2 ? 1 : 0];
}

} // namespace

void begin() {
    if(opened) return;
    opened = store.begin(NAMESPACE, false);
}

int8_t repeaterIndex() {
    if(!opened) return INDEX_UNSET;
    const int8_t value = static_cast<int8_t>(store.getChar(KEY_INDEX, INDEX_UNSET));
    return (value >= 1 && value <= 6) ? value : INDEX_UNSET;
}

bool setRepeaterIndex(int8_t index) {
    if(index < 0 || index > 6) return false;
    if(!opened) return false;
    store.putChar(KEY_INDEX, static_cast<char>(index));
    return true;
}

uint8_t learnedRangeStart() {
    if(!opened) return 0;
    return store.getUChar(KEY_RANGE, 0);
}

void setLearnedRangeStart(uint8_t rangeStart) {
    if(!opened) return;
    if(store.getUChar(KEY_RANGE, 0) == rangeStart) return; // no flash write for no change
    store.putUChar(KEY_RANGE, rangeStart);
}

uint32_t bootCount() { return opened ? store.getULong(KEY_BOOTS, 0) : 0; }

uint32_t unhealthyBoots() { return opened ? store.getULong(KEY_UNHEALTHY, 0) : 0; }

void noteBootAttempt() {
    if(!opened) return;
    store.putULong(KEY_BOOTS, store.getULong(KEY_BOOTS, 0) + 1);
    store.putULong(KEY_UNHEALTHY, store.getULong(KEY_UNHEALTHY, 0) + 1);
}

void noteHealthy() {
    if(!opened) return;
    if(store.getULong(KEY_UNHEALTHY, 0) == 0) return;
    store.putULong(KEY_UNHEALTHY, 0);
}

uint8_t polarityMode(uint8_t side, uint8_t fallback) {
    if(!opened) return fallback;
    const uint8_t value = store.getUChar(sideKey(KEY_POLARITY_MODE, side), fallback);
    return value <= 2 ? value : fallback;
}

void setPolarityMode(uint8_t side, uint8_t mode) {
    if(!opened || mode > 2) return;
    const char* key = sideKey(KEY_POLARITY_MODE, side);
    if(store.isKey(key) && store.getUChar(key, 0xFF) == mode) return;
    store.putUChar(key, mode);
}

bool polarityInverted(uint8_t side, bool fallback) {
    if(!opened) return fallback;
    return store.getBool(sideKey(KEY_POLARITY_INVERTED, side), fallback);
}

void setPolarityInverted(uint8_t side, bool inverted) {
    if(!opened) return;
    const char* key = sideKey(KEY_POLARITY_INVERTED, side);
    if(store.isKey(key) && store.getBool(key, !inverted) == inverted) return;
    store.putBool(key, inverted);
}

bool hardwareDriverEnable(bool fallback) {
    if(!opened) return fallback;
    return store.getBool(KEY_HARDWARE_DE, fallback);
}

void setHardwareDriverEnable(bool enabled) {
    if(!opened) return;
    if(store.isKey(KEY_HARDWARE_DE) && store.getBool(KEY_HARDWARE_DE, !enabled) == enabled) return;
    store.putBool(KEY_HARDWARE_DE, enabled);
}

} // namespace persistence
} // namespace repeater

#endif
