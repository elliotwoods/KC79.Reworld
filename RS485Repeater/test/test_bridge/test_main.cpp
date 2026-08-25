#include <unity.h>

#include <cstdint>
#include <vector>

#include "BridgeCore.h"

using repeater::FrameRouter;
using repeater::FrameView;
using repeater::RoutingMode;
using repeater::Side;

namespace {

std::vector<uint8_t> cobsFrame(const std::vector<uint8_t>& raw) {
    std::vector<uint8_t> encoded;
    encoded.reserve(raw.size() + raw.size() / 254 + 2);
    size_t codeIndex = 0;
    encoded.push_back(0);
    uint8_t code = 1;
    for(uint8_t value : raw) {
        if(value == 0) {
            encoded[codeIndex] = code;
            codeIndex = encoded.size();
            encoded.push_back(0);
            code = 1;
        }
        else {
            encoded.push_back(value);
            if(++code == 0xFF) {
                encoded[codeIndex] = code;
                codeIndex = encoded.size();
                encoded.push_back(0);
                code = 1;
            }
        }
    }
    encoded[codeIndex] = code;
    encoded.push_back(0);
    return encoded;
}

void appendInteger(std::vector<uint8_t>& raw, int value) {
    if(value >= 0 && value <= 127) raw.push_back(static_cast<uint8_t>(value));
    else {
        raw.push_back(0xD0);
        raw.push_back(static_cast<uint8_t>(static_cast<int8_t>(value)));
    }
}

std::vector<uint8_t> envelope(int target, int source, uint8_t body = 0xC0) {
    std::vector<uint8_t> raw{0x93};
    appendInteger(raw, target);
    appendInteger(raw, source);
    raw.push_back(body);
    return cobsFrame(raw);
}

std::vector<uint8_t> keyframe(uint8_t start, uint8_t count) {
    std::vector<uint8_t> raw{0x93};
    appendInteger(raw, -1);
    appendInteger(raw, 0);
    raw.insert(raw.end(), {
        0x81, 0xA8, 'k', 'e', 'y', 'f', 'r', 'a', 'm', 'e',
        0x82, 0xAA, 's', 't', 'a', 'r', 't', 'I', 'n', 'd', 'e', 'x', start,
        0xA6, 'v', 'a', 'l', 'u', 'e', 's'
    });
    raw.push_back(static_cast<uint8_t>(0x90 | count));
    for(uint8_t i = 0; i < count; ++i) raw.insert(raw.end(), {0x92, i, i});
    return cobsFrame(raw);
}

void ingest(FrameRouter& router, Side side, const std::vector<uint8_t>& frame, uint32_t now = 1) {
    router.ingest(side, frame.data(), frame.size(), now);
}

bool pop(FrameRouter& router, Side expectedSource) {
    FrameView view;
    if(!router.nextFrame(view)) return false;
    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(expectedSource), static_cast<uint8_t>(view.source));
    TEST_ASSERT_GREATER_THAN_UINT32(0, view.size);
    router.completeTransmission(view.source);
    return true;
}

void learn(FrameRouter& router, uint8_t source) {
    ingest(router, Side::Two, envelope(0, source, 0xC3));
    TEST_ASSERT_TRUE(pop(router, Side::Two));
}

} // namespace

void test_complete_frames_are_stored_before_forwarding() {
    FrameRouter router;
    const auto frame = envelope(1, 0);
    router.ingest(Side::One, frame.data(), frame.size() - 1, 1);
    FrameView view;
    TEST_ASSERT_FALSE(router.nextFrame(view));
    router.ingest(Side::One, &frame.back(), 1, 2);
    TEST_ASSERT_TRUE(router.nextFrame(view));
    TEST_ASSERT_EQUAL_UINT32(frame.size(), view.size);
    TEST_ASSERT_EQUAL_UINT8_ARRAY(frame.data(), view.data, frame.size());
    router.completeTransmission(Side::One);
    TEST_ASSERT_EQUAL_UINT64(1, router.stats().oneToTwo.forwardedFrames);
}

void test_incomplete_and_oversized_frames_are_dropped_atomically() {
    FrameRouter router(2000);
    const uint8_t partial[] = {2, 0x91};
    router.ingest(Side::One, partial, sizeof(partial), 100);
    router.expireIncomplete(2100);
    TEST_ASSERT_EQUAL_UINT64(1, router.stats().oneToTwo.incompleteFrames);

    std::vector<uint8_t> huge(repeater::MAX_FRAME_BYTES + 1, 0x7F);
    huge.push_back(0);
    router.ingest(Side::One, huge.data(), huge.size(), 3000);
    TEST_ASSERT_EQUAL_UINT64(1, router.stats().oneToTwo.oversizedFrames);
    FrameView view;
    TEST_ASSERT_FALSE(router.nextFrame(view));
}

void test_queue_is_bounded_and_observable() {
    FrameRouter router;
    const auto malformed = cobsFrame({0x01});
    for(size_t i = 0; i < repeater::FRAME_QUEUE_DEPTH + 1; ++i) ingest(router, Side::One, malformed);
    TEST_ASSERT_EQUAL_UINT32(repeater::FRAME_QUEUE_DEPTH, router.queueDepth(Side::One));
    TEST_ASSERT_EQUAL_UINT32(repeater::FRAME_QUEUE_DEPTH, router.stats().oneToTwo.queueHighWater);
    TEST_ASSERT_EQUAL_UINT64(1, router.stats().oneToTwo.queueDrops);
}

void test_each_valid_reply_learns_its_nine_id_block() {
    for(uint8_t block = 0; block < 6; ++block) {
        FrameRouter router;
        const uint8_t source = static_cast<uint8_t>(block * 9 + 5);
        learn(router, source);
        TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(RoutingMode::Filtered), static_cast<uint8_t>(router.routingMode()));
        TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(block * 9 + 1), router.localRangeStart());
        TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(block * 9 + 9), router.localRangeEnd());
    }
}

void test_learned_router_filters_nonlocal_unicasts() {
    FrameRouter router;
    learn(router, 4);
    ingest(router, Side::One, envelope(8, 0));
    TEST_ASSERT_TRUE(pop(router, Side::One));
    ingest(router, Side::One, envelope(10, 0));
    FrameView view;
    TEST_ASSERT_FALSE(router.nextFrame(view));
    TEST_ASSERT_EQUAL_UINT64(1, router.stats().filteredUnicasts);
}

void test_keyframes_are_filtered_by_interval_intersection() {
    FrameRouter router;
    learn(router, 12); // local 10..18
    ingest(router, Side::One, keyframe(1, 9));
    FrameView view;
    TEST_ASSERT_FALSE(router.nextFrame(view));
    ingest(router, Side::One, keyframe(9, 9));
    TEST_ASSERT_TRUE(pop(router, Side::One));
    ingest(router, Side::One, keyframe(10, 9));
    TEST_ASSERT_TRUE(pop(router, Side::One));
    ingest(router, Side::One, keyframe(19, 9));
    TEST_ASSERT_FALSE(router.nextFrame(view));
    TEST_ASSERT_EQUAL_UINT64(2, router.stats().filteredKeyframes);
}

void test_unknown_and_firmware_broadcasts_fail_open() {
    FrameRouter router;
    learn(router, 1);
    ingest(router, Side::One, envelope(-1, 0, 0xA2)); // malformed body, still passed
    TEST_ASSERT_TRUE(pop(router, Side::One));

    std::vector<uint8_t> fwRaw{0x93, 0xD0, 0xFF, 0xD0, 0x00, 0xA2, 'F', 'W'};
    ingest(router, Side::One, cobsFrame(fwRaw));
    TEST_ASSERT_TRUE(pop(router, Side::One));

    std::vector<uint8_t> fwData{
        0x93, 0xD0, 0xFF, 0xD0, 0x00, 0x81, 0x00, 0xC4, 0x04, 0, 0, 0xAA, 0x55
    };
    ingest(router, Side::One, cobsFrame(fwData));
    TEST_ASSERT_TRUE(pop(router, Side::One));
    TEST_ASSERT_EQUAL_UINT64(1, router.stats().parseErrors); // only the deliberately malformed body
}

void test_outer_host_frames_do_not_enter_local_branch() {
    FrameRouter router;
    ingest(router, Side::One, envelope(0, 7, 0xC3));
    FrameView view;
    TEST_ASSERT_FALSE(router.nextFrame(view));
    TEST_ASSERT_EQUAL_UINT64(1, router.stats().filteredHostFrames);
}

void test_conflicting_local_reply_disables_filtering_until_relearn() {
    FrameRouter router;
    learn(router, 1);
    learn(router, 10);
    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(RoutingMode::Conflict), static_cast<uint8_t>(router.routingMode()));
    ingest(router, Side::One, envelope(54, 0));
    TEST_ASSERT_TRUE(pop(router, Side::One));
    router.relearn();
    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(RoutingMode::Transparent), static_cast<uint8_t>(router.routingMode()));
}

void test_out_of_topology_source_is_an_observable_conflict() {
    FrameRouter router;
    learn(router, 1);
    learn(router, 55);
    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(RoutingMode::Conflict), static_cast<uint8_t>(router.routingMode()));
    TEST_ASSERT_EQUAL_UINT64(1, router.stats().topologyConflicts);
}

int main(int, char**) {
    UNITY_BEGIN();
    RUN_TEST(test_complete_frames_are_stored_before_forwarding);
    RUN_TEST(test_incomplete_and_oversized_frames_are_dropped_atomically);
    RUN_TEST(test_queue_is_bounded_and_observable);
    RUN_TEST(test_each_valid_reply_learns_its_nine_id_block);
    RUN_TEST(test_learned_router_filters_nonlocal_unicasts);
    RUN_TEST(test_keyframes_are_filtered_by_interval_intersection);
    RUN_TEST(test_unknown_and_firmware_broadcasts_fail_open);
    RUN_TEST(test_outer_host_frames_do_not_enter_local_branch);
    RUN_TEST(test_conflicting_local_reply_disables_filtering_until_relearn);
    RUN_TEST(test_out_of_topology_source_is_an_observable_conflict);
    return UNITY_END();
}
