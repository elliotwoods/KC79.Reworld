#pragma once

/// Per-side UART polarity, decided at run time from what the wire actually delivers.
///
/// A/B labels are not portable between RS485 vendors, and a pair that is landed the wrong way
/// round is a hardware fact an installation may not be able to change. Measured on the bench
/// 2026-08-26: the host adapter on side 1 was wired with its pair swapped relative to the
/// repeater's transceiver, and with both UARTs at normal polarity every host frame arrived as
/// exactly one UART error and zero bytes, while everything the repeater relayed upstream reached
/// the host as an inverted-polarity garble. Neither firmware nor host could see which.
///
/// The ESP32 UART can invert RXD and TXD in hardware, so the repeater does not need to know
/// the wiring in advance. Each side runs one of these hunters: while it has never decoded a
/// frame it flips polarity whenever a burst of undecodable traffic arrives, and once frames
/// decode it locks. A locked side only re-hunts on strong evidence -- a long run of errored
/// traffic with nothing valid in it -- so a glitch cannot flip a working side.
///
/// Evidence is fed as cumulative counters and the hunter works in deltas, so it costs nothing
/// on the data path and can be driven entirely from the native tests.

#include <cstdint>

namespace repeater {

enum class PolarityMode : uint8_t {
    /// Never inverted. The production default before this existed.
    Normal = 0,
    /// Always inverted. A documented installation override.
    Inverted = 1,
    /// Decide from the traffic. The default now.
    Auto = 2,
};

const char* polarityModeName(PolarityMode mode);
bool polarityModeFromName(const char* name, PolarityMode& out);
bool polarityModeFromValue(int value, PolarityMode& out);

/// Cumulative counters for one side. `rxBytes` and `uartErrors` must exclude the post-transmit
/// shadow window (see `TURNAROUND_SHADOW_US` in the firmware): the byte a floating line
/// manufactures when this side's own driver lets go is not evidence about the wiring.
struct PolarityEvidence {
    uint64_t rxBytes = 0;
    uint64_t validFrames = 0;
    uint64_t uartErrors = 0;
};

struct PolarityConfig {
    /// Minimum time between two flips. Long enough for a host to have sent more than one
    /// frame at the new polarity, short enough that a wrong guess costs one exchange.
    uint32_t dwellMs = 200;
    /// Errors that flip a side which has never decoded anything at this polarity. Two, not
    /// one: a single error is what a stray edge looks like.
    uint32_t huntErrors = 2;
    /// Valid frames that lock the current polarity.
    uint32_t lockFrames = 2;
    /// Errors, with no valid frame among them, that unlock a locked side. Twelve is more
    /// than any burst of noise seen on the bench and less than one second of a host talking
    /// at the wrong polarity.
    uint32_t unlockErrors = 12;
};

class PolarityHunter {
public:
    explicit PolarityHunter(PolarityConfig config = PolarityConfig{});

    /// Set the mode and the starting polarity. The evidence baseline restarts here.
    void configure(PolarityMode mode, bool inverted, const PolarityEvidence& now, uint32_t nowMs);

    /// Feed the counters. Returns true when the caller must apply `inverted()` to the UART.
    /// Counters that go backwards (a counter reset) simply rebase the window.
    bool observe(const PolarityEvidence& now, uint32_t nowMs);

    PolarityMode mode() const { return mode_; }
    bool inverted() const { return inverted_; }
    /// The current polarity has decoded frames since it was last set.
    bool locked() const { return locked_; }
    uint32_t flips() const { return flips_; }

    /// True once after the observation that locked the polarity, so the caller can persist
    /// the value the wire has just proven.
    bool takeLockEvent();

private:
    PolarityConfig config_;
    PolarityMode mode_ = PolarityMode::Normal;
    bool inverted_ = false;
    bool locked_ = false;
    bool lockEvent_ = false;
    uint32_t flips_ = 0;
    uint32_t lastFlipMs_ = 0;
    bool flipped_ = false;
    PolarityEvidence baseline_;
};

} // namespace repeater
