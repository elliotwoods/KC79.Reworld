#include "Polarity.h"

#include <cstring>

namespace repeater {

const char* polarityModeName(PolarityMode mode) {
    switch(mode) {
    case PolarityMode::Normal: return "normal";
    case PolarityMode::Inverted: return "inverted";
    case PolarityMode::Auto: return "auto";
    }
    return "normal";
}

bool polarityModeFromName(const char* name, PolarityMode& out) {
    if(name == nullptr) return false;
    if(std::strcmp(name, "normal") == 0) { out = PolarityMode::Normal; return true; }
    if(std::strcmp(name, "inverted") == 0) { out = PolarityMode::Inverted; return true; }
    if(std::strcmp(name, "auto") == 0) { out = PolarityMode::Auto; return true; }
    return false;
}

bool polarityModeFromValue(int value, PolarityMode& out) {
    switch(value) {
    case 0: out = PolarityMode::Normal; return true;
    case 1: out = PolarityMode::Inverted; return true;
    case 2: out = PolarityMode::Auto; return true;
    default: return false;
    }
}

PolarityHunter::PolarityHunter(PolarityConfig config) : config_(config) { }

void PolarityHunter::configure(PolarityMode mode, bool inverted, const PolarityEvidence& now,
    uint32_t nowMs) {
    mode_ = mode;
    inverted_ = inverted;
    locked_ = false;
    lockEvent_ = false;
    baseline_ = now;
    // A configure counts as a flip for dwell purposes: the wire needs time to show what it
    // thinks of the new setting before it is changed again.
    lastFlipMs_ = nowMs;
    flipped_ = true;
}

bool PolarityHunter::observe(const PolarityEvidence& now, uint32_t nowMs) {
    if(mode_ != PolarityMode::Auto) return false;

    if(now.rxBytes < baseline_.rxBytes || now.validFrames < baseline_.validFrames
        || now.uartErrors < baseline_.uartErrors) {
        // Counters were reset underneath us. Nothing has been learned or unlearned.
        baseline_ = now;
        return false;
    }

    const uint64_t frames = now.validFrames - baseline_.validFrames;
    const uint64_t errors = now.uartErrors - baseline_.uartErrors;

    if(frames >= (locked_ ? 1u : config_.lockFrames)) {
        // This polarity decodes. Lock it, or keep it locked, and forget the errors that came
        // before the frames -- they were the price of finding it, not a reason to leave it.
        if(!locked_) {
            locked_ = true;
            lockEvent_ = true;
        }
        baseline_ = now;
        return false;
    }

    const uint64_t needed = locked_ ? config_.unlockErrors : config_.huntErrors;
    if(errors < needed) return false;
    if(flipped_ && static_cast<uint32_t>(nowMs - lastFlipMs_) < config_.dwellMs) return false;

    inverted_ = !inverted_;
    locked_ = false;
    lockEvent_ = false;
    flips_++;
    lastFlipMs_ = nowMs;
    flipped_ = true;
    baseline_ = now;
    return true;
}

bool PolarityHunter::takeLockEvent() {
    const bool event = lockEvent_;
    lockEvent_ = false;
    return event;
}

} // namespace repeater
