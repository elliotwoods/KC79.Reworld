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

Preferences store;
bool opened = false;

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

} // namespace persistence
} // namespace repeater

#endif
