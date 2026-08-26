#include <unity.h>

#include <cstdint>
#include <vector>

#include "BridgeCore.h"

using repeater::FrameRouter;
using repeater::FrameView;
using repeater::BlockState;
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

/// Give the panel the block its provisioned index implies. Replaces the old helper that
/// taught the router by replaying a branch reply -- traffic no longer teaches it anything.
void learn(FrameRouter& router, uint8_t source) {
    router.setLocalBlock(static_cast<uint8_t>(((source - 1) / 9) * 9 + 1));
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

void test_the_local_block_comes_from_the_provisioned_index() {
    for(uint8_t panel = 0; panel < 6; ++panel) {
        FrameRouter router;
        TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(BlockState::Unknown), static_cast<uint8_t>(router.blockState()));
        router.setLocalBlock(static_cast<uint8_t>(panel * 9 + 1));
        TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(BlockState::Assigned), static_cast<uint8_t>(router.blockState()));
        TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(panel * 9 + 1), router.localRangeStart());
        TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(panel * 9 + 9), router.localRangeEnd());
    }
}

/// Panels are chained, so a unicast for a Portal on a panel further down the chain has
/// no route to it except through this one. Dropping it -- which a star topology could
/// afford, because every repeater heard the host directly -- strands everything below.
void test_unicasts_for_downstream_panels_are_relayed() {
    FrameRouter router;
    learn(router, 4); // this panel serves 1..9
    ingest(router, Side::One, envelope(8, 0));
    TEST_ASSERT_TRUE(pop(router, Side::One));
    ingest(router, Side::One, envelope(10, 0)); // panel 2
    TEST_ASSERT_TRUE(pop(router, Side::One));
    ingest(router, Side::One, envelope(54, 0)); // panel 6, the far end
    TEST_ASSERT_TRUE(pop(router, Side::One));
}

/// Same argument for batched motion: a keyframe block addressed at panels below this one
/// crosses this panel's bus on its way there.
void test_keyframes_for_other_panels_are_relayed() {
    FrameRouter router;
    learn(router, 12);
    for(uint8_t start : {uint8_t{1}, uint8_t{9}, uint8_t{10}, uint8_t{19}, uint8_t{46}}) {
        ingest(router, Side::One, keyframe(start, 9));
        TEST_ASSERT_TRUE(pop(router, Side::One));
    }
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

/// A panel sees every downstream panel's replies crossing its own bus on the way up.
/// Those must not touch its idea of which nine boards are its own -- inferring the block
/// from traffic used to adopt whichever panel answered first, then call the next one a
/// topology conflict.
void test_transit_replies_from_other_panels_leave_the_block_alone() {
    FrameRouter router;
    router.setLocalBlock(1);
    for(uint8_t source : {uint8_t{12}, uint8_t{30}, uint8_t{54}, uint8_t{5}}) {
        ingest(router, Side::Two, envelope(0, source, 0xC3));
        TEST_ASSERT_TRUE(pop(router, Side::Two)); // relayed upstream, unread
    }
    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(BlockState::Assigned), static_cast<uint8_t>(router.blockState()));
    TEST_ASSERT_EQUAL_UINT8(1, router.localRangeStart());
    TEST_ASSERT_EQUAL_UINT8(9, router.localRangeEnd());
}


/// A host-addressed frame that is not repeater-plane traffic is a Portal reply that has
/// come the wrong way. It must never enter a branch, whether or not this panel knows its
/// own block.
///
/// Control-plane frames are the opposite case and are covered below: they used to be
/// dropped here too, which is what made every panel past the first unreachable.
void test_stray_host_addressed_frames_never_enter_a_branch() {
    for(bool assigned : {false, true}) {
        FrameRouter router;
        if(assigned) router.setLocalBlock(1);
        ingest(router, Side::One, envelope(0, 0, 0xC3));
        FrameView view;
        TEST_ASSERT_FALSE(router.nextFrame(view));
        TEST_ASSERT_EQUAL_UINT64(1, router.stats().filteredHostFrames);
    }
}

/// The change the chain forced: a request for a panel below this one is relayed rather
/// than swallowed, and a broadcast is both acted on here and passed along.
void test_control_frames_for_other_panels_are_relayed() {
    class Router : public repeater::ControlFrameConsumer {
    public:
        explicit Router(repeater::ControlDisposition d) : disposition(d) { }
        repeater::ControlDisposition consumeControlFrame(const uint8_t*, size_t) override {
            seen++;
            return disposition;
        }
        repeater::ControlDisposition disposition;
        int seen = 0;
    };

    {
        Router consumer(repeater::ControlDisposition::Relay);
        FrameRouter router;
        router.setControlFrameConsumer(&consumer);
        ingest(router, Side::One, envelope(0, 0, 0xC3));
        TEST_ASSERT_TRUE(pop(router, Side::One));
        TEST_ASSERT_EQUAL_INT(1, consumer.seen);
        TEST_ASSERT_EQUAL_UINT64(1, router.stats().relayedControlFrames);
        TEST_ASSERT_EQUAL_UINT64(0, router.stats().filteredHostFrames);
    }
    {
        Router consumer(repeater::ControlDisposition::ConsumedAndRelay);
        FrameRouter router;
        router.setControlFrameConsumer(&consumer);
        ingest(router, Side::One, envelope(0, 0, 0xC3));
        TEST_ASSERT_TRUE(pop(router, Side::One));
        TEST_ASSERT_EQUAL_UINT64(1, router.stats().controlFrames);
    }
    {
        // Addressed to this panel: acted on, and it stops here.
        Router consumer(repeater::ControlDisposition::Consumed);
        FrameRouter router;
        router.setControlFrameConsumer(&consumer);
        ingest(router, Side::One, envelope(0, 0, 0xC3));
        FrameView view;
        TEST_ASSERT_FALSE(router.nextFrame(view));
        TEST_ASSERT_EQUAL_UINT64(1, router.stats().controlFrames);
    }
}

// PortalFW already emits `[target, source, body, seq, crc16]`. Every decoder in the
// system tolerates the extra elements; nothing pinned that the repeater does too.
void test_five_element_envelopes_are_inspected_normally() {
    FrameRouter router;
    learn(router, 1);

    std::vector<uint8_t> raw{0x95};
    appendInteger(raw, 20); // a Portal on a panel further down the chain
    appendInteger(raw, 0);
    raw.push_back(0xC0);
    raw.push_back(0x07);             // seq
    raw.insert(raw.end(), {0xCD, 0x29, 0xB1}); // crc16
    ingest(router, Side::One, cobsFrame(raw));

    TEST_ASSERT_TRUE(pop(router, Side::One)); // parsed, and relayed down the chain
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

    // An unclaimed reply, and any non-position traffic, still reaches the host.
    ingest(router, Side::Two, positionReply(5));
    TEST_ASSERT_TRUE(pop(router, Side::Two));
    ingest(router, Side::Two, envelope(0, 4, 0xC3));
    TEST_ASSERT_TRUE(pop(router, Side::Two));
    TEST_ASSERT_EQUAL_UINT64(1, router.stats().consumedInnerFrames);
}

void test_a_block_start_must_be_a_legal_one() {
    for(uint8_t start : {uint8_t{1}, uint8_t{10}, uint8_t{19}, uint8_t{28}, uint8_t{37}, uint8_t{46}}) {
        FrameRouter router;
        router.setLocalBlock(start);
        TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(BlockState::Assigned), static_cast<uint8_t>(router.blockState()));
        TEST_ASSERT_EQUAL_UINT8(start, router.localRangeStart());
        TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(start + 8), router.localRangeEnd());
    }
    for(uint8_t start : {uint8_t{0}, uint8_t{2}, uint8_t{9}, uint8_t{45}, uint8_t{119}, uint8_t{255}}) {
        FrameRouter router;
        router.setLocalBlock(start);
        TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(BlockState::Unknown), static_cast<uint8_t>(router.blockState()));
        TEST_ASSERT_EQUAL_UINT8(0, router.localRangeStart());
    }
}

void test_maintenance_pause_stops_relaying_but_not_the_control_plane() {
    class Claimer : public repeater::ControlFrameConsumer {
    public:
        repeater::ControlDisposition consumeControlFrame(const uint8_t*, size_t) override {
            seen++;
            return repeater::ControlDisposition::Consumed;
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

/// A frame that arrives in two pieces is still one frame.
///
/// This is the bench failure of 2026-08-26 in miniature. The host writes a frame with one
/// `write_all`, but the USB-serial path delivers it in 64-byte packets, and once a frame spans
/// more than two of them the gap in the middle exceeded the old 2 ms discard timer. `ingest` and
/// `expireIncomplete` both run every loop pass, so the timer fired between the halves and the
/// frame was dropped with nothing on the wire having gone wrong. Measured: 159 bytes relayed,
/// 160 bytes silently discarded, and an entire firmware transfer lost to it.
void test_a_bare_delimiter_is_absorbed_not_relayed() {
    // A delimiter with nothing in front of it closes nothing. It used to become a one-byte
    // frame: counted as received, counted as a parse error, and relayed to the branch under
    // its own driver-enable interval -- which manufactured a fresh turn-around glitch directly
    // in front of the real frame the delimiter existed to protect.
    FrameRouter router;
    const uint8_t zero = 0;
    router.ingest(Side::One, &zero, 1, 1);
    FrameView view;
    TEST_ASSERT_FALSE(router.nextFrame(view));
    TEST_ASSERT_EQUAL_UINT64(1, router.stats().oneToTwo.emptyFrames);
    TEST_ASSERT_EQUAL_UINT64(0, router.stats().oneToTwo.receivedFrames);
    TEST_ASSERT_EQUAL_UINT64(0, router.stats().parseErrors);
    TEST_ASSERT_EQUAL_UINT32(0, router.queueDepth(Side::One));
}

void test_a_leading_delimiter_does_not_split_the_frame_behind_it() {
    // What the host emits once encode_frame writes a delimiter at both ends: the real frame
    // must still relay exactly once, byte-identical, and cost nothing extra.
    FrameRouter router;
    const auto frame = envelope(1, 0);
    std::vector<uint8_t> wire{0};
    wire.insert(wire.end(), frame.begin(), frame.end());
    router.ingest(Side::One, wire.data(), wire.size(), 1);

    FrameView view;
    TEST_ASSERT_TRUE(router.nextFrame(view));
    TEST_ASSERT_EQUAL_UINT32(frame.size(), view.size);
    TEST_ASSERT_EQUAL_UINT8_ARRAY(frame.data(), view.data, frame.size());
    router.completeTransmission(Side::One);
    TEST_ASSERT_FALSE(router.nextFrame(view));
    TEST_ASSERT_EQUAL_UINT64(1, router.stats().oneToTwo.receivedFrames);
    TEST_ASSERT_EQUAL_UINT64(1, router.stats().oneToTwo.emptyFrames);
    TEST_ASSERT_EQUAL_UINT64(0, router.stats().parseErrors);
}

void test_a_run_of_delimiters_between_frames_is_absorbed() {
    FrameRouter router;
    const auto first = envelope(1, 0);
    const auto second = envelope(2, 0);
    std::vector<uint8_t> wire(first.begin(), first.end());
    wire.push_back(0);
    wire.push_back(0);
    wire.push_back(0);
    wire.insert(wire.end(), second.begin(), second.end());
    router.ingest(Side::One, wire.data(), wire.size(), 1);

    FrameView view;
    TEST_ASSERT_TRUE(router.nextFrame(view));
    router.completeTransmission(Side::One);
    TEST_ASSERT_TRUE(router.nextFrame(view));
    router.completeTransmission(Side::One);
    TEST_ASSERT_FALSE(router.nextFrame(view));
    TEST_ASSERT_EQUAL_UINT64(2, router.stats().oneToTwo.receivedFrames);
    TEST_ASSERT_EQUAL_UINT64(3, router.stats().oneToTwo.emptyFrames);
    TEST_ASSERT_EQUAL_UINT64(0, router.stats().parseErrors);
}

void test_a_frame_split_across_a_usb_packet_gap_still_arrives() {
    FrameRouter router(20000);
    auto frame = envelope(1, 0);
    // Padded past the two-USB-packet boundary where the gap appears in practice.
    while(frame.size() < 160) frame.insert(frame.end() - 1, 0x7F);

    const size_t half = 64;
    router.ingest(Side::One, frame.data(), half, 1000);
    // Several loop passes with nothing arriving -- more than the old 2 ms, less than the new 20 ms.
    router.expireIncomplete(4000);
    router.ingest(Side::One, frame.data() + half, frame.size() - half, 6000);

    FrameView view;
    TEST_ASSERT_TRUE_MESSAGE(router.nextFrame(view), "a split frame was discarded");
    TEST_ASSERT_EQUAL_UINT32(frame.size(), view.size);
    TEST_ASSERT_EQUAL_UINT8_ARRAY(frame.data(), view.data, frame.size());
    TEST_ASSERT_EQUAL_UINT64(0, router.stats().oneToTwo.incompleteFrames);
}

/// The longer timer must still abandon a stream that really did stop.
void test_a_stream_that_stops_is_still_abandoned() {
    FrameRouter router(20000);
    const uint8_t partial[] = {2, 0x91};
    router.ingest(Side::One, partial, sizeof(partial), 1000);

    router.expireIncomplete(15000);
    TEST_ASSERT_EQUAL_UINT64_MESSAGE(0, router.stats().oneToTwo.incompleteFrames,
        "abandoned too early -- this is the gap a split frame lives in");

    router.expireIncomplete(21001);
    TEST_ASSERT_EQUAL_UINT64(1, router.stats().oneToTwo.incompleteFrames);
}

int main(int, char**) {
    UNITY_BEGIN();
    RUN_TEST(test_complete_frames_are_stored_before_forwarding);
    RUN_TEST(test_incomplete_and_oversized_frames_are_dropped_atomically);
    RUN_TEST(test_a_bare_delimiter_is_absorbed_not_relayed);
    RUN_TEST(test_a_leading_delimiter_does_not_split_the_frame_behind_it);
    RUN_TEST(test_a_run_of_delimiters_between_frames_is_absorbed);
    RUN_TEST(test_a_frame_split_across_a_usb_packet_gap_still_arrives);
    RUN_TEST(test_a_stream_that_stops_is_still_abandoned);
    RUN_TEST(test_queue_is_bounded_and_observable);
    RUN_TEST(test_the_local_block_comes_from_the_provisioned_index);
    RUN_TEST(test_unicasts_for_downstream_panels_are_relayed);
    RUN_TEST(test_keyframes_for_other_panels_are_relayed);
    RUN_TEST(test_unknown_and_firmware_broadcasts_fail_open);
    RUN_TEST(test_outer_host_frames_do_not_enter_local_branch);
    RUN_TEST(test_transit_replies_from_other_panels_leave_the_block_alone);
    RUN_TEST(test_stray_host_addressed_frames_never_enter_a_branch);
    RUN_TEST(test_control_frames_for_other_panels_are_relayed);
    RUN_TEST(test_five_element_envelopes_are_inspected_normally);
    RUN_TEST(test_originated_frames_are_transmitted_ahead_of_relayed_traffic);
    RUN_TEST(test_oversized_or_overflowing_originations_are_refused);
    RUN_TEST(test_claimed_inner_replies_are_not_relayed_upstream);
    RUN_TEST(test_a_block_start_must_be_a_legal_one);
    RUN_TEST(test_maintenance_pause_stops_relaying_but_not_the_control_plane);
    return UNITY_END();
}
