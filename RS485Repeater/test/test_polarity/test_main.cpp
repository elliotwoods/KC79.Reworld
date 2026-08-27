#include <unity.h>

#include <cstdint>
#include <initializer_list>

#include "Polarity.h"

using repeater::PolarityConfig;
using repeater::PolarityEvidence;
using repeater::PolarityHunter;
using repeater::PolarityMode;

namespace {

PolarityEvidence evidence(uint64_t bytes, uint64_t frames, uint64_t errors) {
    PolarityEvidence e;
    e.rxBytes = bytes;
    e.validFrames = frames;
    e.uartErrors = errors;
    return e;
}

PolarityHunter autoHunter(bool inverted = false, uint32_t now = 0) {
    PolarityHunter hunter;
    hunter.configure(PolarityMode::Auto, inverted, PolarityEvidence{}, now);
    return hunter;
}

} // namespace

void setUp() { }
void tearDown() { }

/// The bench case: every host frame arrives as one UART error and nothing else.
void test_errored_traffic_with_nothing_valid_flips_an_unlocked_side() {
    PolarityHunter hunter = autoHunter(false, 0);
    // One error is a stray edge, not evidence.
    TEST_ASSERT_FALSE(hunter.observe(evidence(0, 0, 1), 1000));
    TEST_ASSERT_FALSE(hunter.inverted());
    // The second, once the dwell has passed, is.
    TEST_ASSERT_TRUE(hunter.observe(evidence(0, 0, 2), 1100));
    TEST_ASSERT_TRUE(hunter.inverted());
    TEST_ASSERT_EQUAL_UINT32(1, hunter.flips());
    TEST_ASSERT_FALSE(hunter.locked());
}

void test_valid_frames_lock_the_polarity_and_report_it_once() {
    PolarityHunter hunter = autoHunter(true, 0);
    TEST_ASSERT_FALSE(hunter.observe(evidence(40, 1, 0), 500));
    TEST_ASSERT_FALSE(hunter.locked());
    TEST_ASSERT_FALSE(hunter.observe(evidence(80, 2, 0), 600));
    TEST_ASSERT_TRUE(hunter.locked());
    TEST_ASSERT_TRUE(hunter.inverted());
    TEST_ASSERT_TRUE(hunter.takeLockEvent());
    TEST_ASSERT_FALSE(hunter.takeLockEvent());
    TEST_ASSERT_EQUAL_UINT32(0, hunter.flips());
}

/// A firmware upload on the branch side: thousands of frames go out, nothing comes back for
/// minutes, and the only inbound bytes are turnaround glitches -- which the caller has already
/// excluded from the evidence. Nothing may flip.
void test_a_locked_side_ignores_silence_and_the_odd_error() {
    PolarityHunter hunter = autoHunter(false, 0);
    hunter.observe(evidence(100, 3, 0), 100);
    TEST_ASSERT_TRUE(hunter.locked());
    for(uint32_t t = 200; t < 60000; t += 100) {
        // Eleven errors spread over a minute, no frames: still under the unlock threshold.
        const uint64_t errors = (t / 5000) < 11 ? t / 5000 : 11;
        TEST_ASSERT_FALSE(hunter.observe(evidence(100 + t / 100, 3, errors), t));
    }
    TEST_ASSERT_TRUE(hunter.locked());
    TEST_ASSERT_FALSE(hunter.inverted());
}

/// Re-wired while running: a locked side that then sees a solid run of errored traffic with
/// no valid frame in it re-hunts.
void test_a_locked_side_unlocks_on_a_long_run_of_undecodable_traffic() {
    PolarityHunter hunter = autoHunter(false, 0);
    hunter.observe(evidence(100, 3, 0), 100);
    TEST_ASSERT_TRUE(hunter.locked());
    TEST_ASSERT_FALSE(hunter.observe(evidence(100, 3, 11), 5000));
    TEST_ASSERT_TRUE(hunter.observe(evidence(100, 3, 12), 5100));
    TEST_ASSERT_TRUE(hunter.inverted());
    TEST_ASSERT_FALSE(hunter.locked());
    TEST_ASSERT_EQUAL_UINT32(1, hunter.flips());
}

/// Errors that arrived before the frames that locked the side are forgotten, so a side that
/// was found by hunting does not carry its search cost forward as unlock evidence.
void test_locking_forgets_the_errors_that_preceded_it() {
    PolarityHunter hunter = autoHunter(false, 0);
    TEST_ASSERT_TRUE(hunter.observe(evidence(0, 0, 5), 1000)); // flip after a burst
    TEST_ASSERT_FALSE(hunter.observe(evidence(0, 0, 9), 1100)); // more errors, inside the dwell
    hunter.observe(evidence(60, 2, 9), 1150);                   // then it decodes
    TEST_ASSERT_TRUE(hunter.locked());
    // Eleven more errors from here are not twelve from the lock.
    TEST_ASSERT_FALSE(hunter.observe(evidence(60, 2, 20), 3000));
    TEST_ASSERT_TRUE(hunter.locked());
}

void test_flips_are_rate_limited_by_the_dwell() {
    PolarityConfig config;
    config.dwellMs = 500;
    PolarityHunter hunter(config);
    hunter.configure(PolarityMode::Auto, false, PolarityEvidence{}, 0);
    TEST_ASSERT_FALSE(hunter.observe(evidence(0, 0, 4), 100)); // inside the dwell after configure
    TEST_ASSERT_TRUE(hunter.observe(evidence(0, 0, 4), 500));
    TEST_ASSERT_FALSE(hunter.observe(evidence(0, 0, 8), 600)); // inside the dwell after the flip
    TEST_ASSERT_TRUE(hunter.observe(evidence(0, 0, 8), 1000));
    TEST_ASSERT_FALSE(hunter.inverted());
    TEST_ASSERT_EQUAL_UINT32(2, hunter.flips());
}

void test_fixed_modes_never_flip() {
    for(PolarityMode mode : {PolarityMode::Normal, PolarityMode::Inverted}) {
        PolarityHunter hunter;
        hunter.configure(mode, mode == PolarityMode::Inverted, PolarityEvidence{}, 0);
        TEST_ASSERT_FALSE(hunter.observe(evidence(0, 0, 100), 10000));
        TEST_ASSERT_FALSE(hunter.observe(evidence(500, 0, 200), 20000));
        TEST_ASSERT_TRUE((mode == PolarityMode::Inverted) == hunter.inverted());
        TEST_ASSERT_EQUAL_UINT32(0, hunter.flips());
    }
}

/// `reset-counters` zeroes the counters the hunter reads. That must rebase, not flip.
void test_a_counter_reset_rebases_rather_than_deciding_anything() {
    PolarityHunter hunter = autoHunter(false, 0);
    hunter.observe(evidence(1000, 30, 7), 1000);
    TEST_ASSERT_TRUE(hunter.locked());
    TEST_ASSERT_FALSE(hunter.observe(evidence(0, 0, 0), 2000));
    TEST_ASSERT_TRUE(hunter.locked());
    TEST_ASSERT_FALSE(hunter.observe(evidence(10, 0, 11), 3000));
    TEST_ASSERT_TRUE(hunter.observe(evidence(10, 0, 12), 3100));
}

void test_mode_names_round_trip() {
    PolarityMode mode;
    TEST_ASSERT_TRUE(repeater::polarityModeFromName("auto", mode));
    TEST_ASSERT_TRUE(mode == PolarityMode::Auto);
    TEST_ASSERT_TRUE(repeater::polarityModeFromName("inverted", mode));
    TEST_ASSERT_TRUE(mode == PolarityMode::Inverted);
    TEST_ASSERT_TRUE(repeater::polarityModeFromName("normal", mode));
    TEST_ASSERT_TRUE(mode == PolarityMode::Normal);
    TEST_ASSERT_FALSE(repeater::polarityModeFromName("sideways", mode));
    TEST_ASSERT_FALSE(repeater::polarityModeFromValue(3, mode));
    TEST_ASSERT_TRUE(repeater::polarityModeFromValue(2, mode));
    TEST_ASSERT_EQUAL_STRING("auto", repeater::polarityModeName(mode));
}

int main(int, char**) {
    UNITY_BEGIN();
    RUN_TEST(test_errored_traffic_with_nothing_valid_flips_an_unlocked_side);
    RUN_TEST(test_valid_frames_lock_the_polarity_and_report_it_once);
    RUN_TEST(test_a_locked_side_ignores_silence_and_the_odd_error);
    RUN_TEST(test_a_locked_side_unlocks_on_a_long_run_of_undecodable_traffic);
    RUN_TEST(test_locking_forgets_the_errors_that_preceded_it);
    RUN_TEST(test_flips_are_rate_limited_by_the_dwell);
    RUN_TEST(test_fixed_modes_never_flip);
    RUN_TEST(test_a_counter_reset_rebases_rather_than_deciding_anything);
    RUN_TEST(test_mode_names_round_trip);
    return UNITY_END();
}
