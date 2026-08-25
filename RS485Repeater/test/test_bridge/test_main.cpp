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

// The repeater control plane addresses a repeater with envelope target 0, relying on
// this drop so that a repeater running older firmware ignores control-plane traffic
// instead of relaying it onto its Portal branch. That makes the behaviour
// load-bearing in every routing mode, not just the filtered one.
void test_host_addressed_frames_are_dropped_in_every_routing_mode() {
    {
        FrameRouter router; // transparent
        TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(RoutingMode::Transparent), static_cast<uint8_t>(router.routingMode()));
        ingest(router, Side::One, envelope(0, 0, 0xC3));
        FrameView view;
        TEST_ASSERT_FALSE(router.nextFrame(view));
        TEST_ASSERT_EQUAL_UINT64(1, router.stats().filteredHostFrames);
    }
    {
        FrameRouter router; // filtered
        learn(router, 1);
        ingest(router, Side::One, envelope(0, 0, 0xC3));
        FrameView view;
        TEST_ASSERT_FALSE(router.nextFrame(view));
        TEST_ASSERT_EQUAL_UINT64(1, router.stats().filteredHostFrames);
    }
    {
        FrameRouter router; // conflict
        learn(router, 1);
        learn(router, 10);
        TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(RoutingMode::Conflict), static_cast<uint8_t>(router.routingMode()));
        ingest(router, Side::One, envelope(0, 0, 0xC3));
        FrameView view;
        TEST_ASSERT_FALSE(router.nextFrame(view));
        TEST_ASSERT_EQUAL_UINT64(1, router.stats().filteredHostFrames);
    }
}

// PortalFW already emits `[target, source, body, seq, crc16]`. Every decoder in the
// system tolerates the extra elements; nothing pinned that the repeater does too.
void test_five_element_envelopes_are_inspected_normally() {
    FrameRouter router;
    learn(router, 1);

    std::vector<uint8_t> raw{0x95};
    appendInteger(raw, 20); // a target outside the learned 1..9 block
    appendInteger(raw, 0);
    raw.push_back(0xC0);
    raw.push_back(0x07);             // seq
    raw.insert(raw.end(), {0xCD, 0x29, 0xB1}); // crc16
    ingest(router, Side::One, cobsFrame(raw));

    FrameView view;
    TEST_ASSERT_FALSE(router.nextFrame(view)); // parsed, and filtered as non-local
    TEST_ASSERT_EQUAL_UINT64(1, router.stats().filteredUnicasts);
    TEST_ASSERT_EQUAL_UINT64(0, router.stats().parseErrors);
}

void test_originated_frames_are_transmitted_ahead_of_relayed_traffic() {
    FrameRouter router;
    const auto relayed = envelope(5, 0);
    ingest(router, Side::One, relayed);

    const auto poll = envelope(3, 0);
    TEST_ASSERT_TRUE(router.originate(Side::Two, poll.data(), poll.size()));
    TEST_ASSERT_EQUAL_UINT32(1, router.originateDepth(Side::Two));

    FrameView view;
    TEST_ASSERT_TRUE(router.nextFrame(view));
    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(Side::None), static_cast<uint8_t>(view.source));
    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(Side::Two), static_cast<uint8_t>(view.destination));
    TEST_ASSERT_EQUAL_UINT32(poll.size(), view.size);
    TEST_ASSERT_EQUAL_UINT8_ARRAY(poll.data(), view.data, poll.size());
    router.completeOriginated(view.destination);
    TEST_ASSERT_EQUAL_UINT64(1, router.stats().originatedFrames);

    // The relayed frame is still queued and goes next, with its destination filled in.
    TEST_ASSERT_TRUE(router.nextFrame(view));
    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(Side::One), static_cast<uint8_t>(view.source));
    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(Side::Two), static_cast<uint8_t>(view.destination));
    router.completeTransmission(view.source);
    TEST_ASSERT_EQUAL_UINT64(0, router.stats().txErrors);
}

void test_oversized_or_overflowing_originations_are_refused() {
    FrameRouter router;
    const std::vector<uint8_t> huge(repeater::MAX_ORIGINATED_FRAME_BYTES + 1, 0x01);
    TEST_ASSERT_FALSE(router.originate(Side::Two, huge.data(), huge.size()));

    const auto poll = envelope(3, 0);
    for(size_t i = 0; i < repeater::ORIGINATE_QUEUE_DEPTH; ++i) {
        TEST_ASSERT_TRUE(router.originate(Side::Two, poll.data(), poll.size()));
    }
    TEST_ASSERT_FALSE(router.originate(Side::Two, poll.data(), poll.size()));
    TEST_ASSERT_EQUAL_UINT64(2, router.stats().originateDrops);
}

namespace {

// Claims position replies from a nominated Portal, the way a snapshot sweep does.
class PositionClaimer : public repeater::InnerReplyConsumer {
public:
    explicit PositionClaimer(int64_t wanted) : wanted_(wanted) { }

    bool consumeInnerReply(const repeater::InnerFrameInfo& info, const uint8_t*, size_t) override {
        if(info.target != 0 || !info.isPositionReply || info.source != wanted_) return false;
        claimed++;
        return true;
    }

    int claimed = 0;

private:
    int64_t wanted_;
};

// `[0, source, {"p": [1, 2, 3, 4]}]` — a Portal position reply.
std::vector<uint8_t> positionReply(int source) {
    std::vector<uint8_t> raw{0x93};
    appendInteger(raw, 0);
    appendInteger(raw, source);
    raw.insert(raw.end(), {0x81, 0xA1, 'p', 0x94, 0x01, 0x02, 0x03, 0x04});
    return cobsFrame(raw);
}

} // namespace

void test_claimed_inner_replies_are_not_relayed_upstream() {
    FrameRouter router;
    PositionClaimer claimer(4);
    router.setInnerReplyConsumer(&claimer);

    ingest(router, Side::Two, positionReply(4));
    FrameView view;
    TEST_ASSERT_FALSE(router.nextFrame(view));
    TEST_ASSERT_EQUAL_INT(1, claimer.claimed);
    TEST_ASSERT_EQUAL_UINT64(1, router.stats().consumedInnerFrames);

    // Range learning still happens for a claimed frame.
    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(RoutingMode::Filtered), static_cast<uint8_t>(router.routingMode()));
    TEST_ASSERT_EQUAL_UINT8(1, router.localRangeStart());

    // An unclaimed reply, and any non-position traffic, still reaches the host.
    ingest(router, Side::Two, positionReply(5));
    TEST_ASSERT_TRUE(pop(router, Side::Two));
    ingest(router, Side::Two, envelope(0, 4, 0xC3));
    TEST_ASSERT_TRUE(pop(router, Side::Two));
    TEST_ASSERT_EQUAL_UINT64(1, router.stats().consumedInnerFrames);
}

void test_restored_range_accepts_only_legal_block_starts() {
    for(uint8_t start : {1, 10, 19, 28, 37, 46}) {
        FrameRouter router;
        router.restoreLearnedRange(start);
        TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(RoutingMode::Filtered), static_cast<uint8_t>(router.routingMode()));
        TEST_ASSERT_EQUAL_UINT8(start, router.localRangeStart());
        TEST_ASSERT_EQUAL_UINT8(start + 8, router.localRangeEnd());
    }
    for(uint8_t start : {0, 2, 9, 45, 55, 255}) {
        FrameRouter router;
        router.restoreLearnedRange(start);
        TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(RoutingMode::Transparent), static_cast<uint8_t>(router.routingMode()));
        TEST_ASSERT_EQUAL_UINT8(0, router.localRangeStart());
    }
}

void test_maintenance_pause_stops_relaying_but_not_the_control_plane() {
    class Claimer : public repeater::ControlFrameConsumer {
    public:
        bool consumeControlFrame(const uint8_t*, size_t) override {
            seen++;
            return true;
        }
        int seen = 0;
    } claimer;

    FrameRouter router;
    router.setControlFrameConsumer(&claimer);
    learn(router, 1);
    router.setForwardingPaused(true);

    FrameView view;
    // Ordinary unicast traffic for this branch is dropped, not queued, so the
    // update does not end with a burst of stale commands.
    ingest(router, Side::One, envelope(3, 0));
    TEST_ASSERT_FALSE(router.nextFrame(view));

    // So is traffic that cannot be decoded at all, which is otherwise fail-open.
    ingest(router, Side::One, cobsFrame({0x01}));
    TEST_ASSERT_FALSE(router.nextFrame(view));

    // And branch replies stop being relayed upstream.
    ingest(router, Side::Two, envelope(0, 4, 0xC3));
    TEST_ASSERT_FALSE(router.nextFrame(view));
    TEST_ASSERT_EQUAL_UINT64(3, router.stats().pausedDrops);

    // But a control frame still reaches the plane, which is what lets a paused
    // repeater be told to resume.
    ingest(router, Side::One, envelope(0, 0, 0xC3));
    TEST_ASSERT_EQUAL_INT(1, claimer.seen);
    TEST_ASSERT_EQUAL_UINT64(1, router.stats().controlFrames);

    router.setForwardingPaused(false);
    ingest(router, Side::One, envelope(3, 0));
    TEST_ASSERT_TRUE(pop(router, Side::One));
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
    RUN_TEST(test_host_addressed_frames_are_dropped_in_every_routing_mode);
    RUN_TEST(test_five_element_envelopes_are_inspected_normally);
    RUN_TEST(test_originated_frames_are_transmitted_ahead_of_relayed_traffic);
    RUN_TEST(test_oversized_or_overflowing_originations_are_refused);
    RUN_TEST(test_claimed_inner_replies_are_not_relayed_upstream);
    RUN_TEST(test_restored_range_accepts_only_legal_block_starts);
    RUN_TEST(test_maintenance_pause_stops_relaying_but_not_the_control_plane);
    return UNITY_END();
}
