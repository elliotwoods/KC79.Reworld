#include <unity.h>

#include <cstdint>
#include <cstring>
#include <vector>

#include "SnapshotEngine.h"
#include "Wire.h"

using repeater::InnerFrameInfo;
using repeater::SnapshotEngine;

namespace {

std::vector<uint8_t> frameOf(const std::vector<uint8_t>& raw) {
    std::vector<uint8_t> out(raw.size() + raw.size() / 254 + 2);
    const size_t size = repeater::wire::cobsEncodeFrame(raw.data(), raw.size(), out.data(), out.size());
    out.resize(size);
    return out;
}

/// `[0, source, {"p": [a, b, ta, tb]}]`, as a Portal answers a position poll.
std::vector<uint8_t> positionReply(int source) {
    uint8_t buffer[64];
    repeater::wire::MsgpackWriter w(buffer, sizeof(buffer));
    w.arrayHeader(3);
    w.integer(0);
    w.integer(source);
    w.mapHeader(1);
    w.key("p");
    w.arrayHeader(4);
    w.integer(source * 10);
    w.integer(source * 20);
    w.integer(source * 30);
    w.integer(source * 40);
    return frameOf(std::vector<uint8_t>(w.data(), w.data() + w.size()));
}

InnerFrameInfo positionInfo(int source) {
    InnerFrameInfo info;
    info.target = 0;
    info.source = source;
    info.isPositionReply = true;
    return info;
}

/// Decodes the polled target ID out of a frame the engine produced.
int polledTarget(const uint8_t* framed, size_t size) {
    uint8_t decoded[64];
    size_t decodedSize = 0;
    TEST_ASSERT_TRUE(repeater::wire::cobsDecode(framed, size - 1, decoded, sizeof(decoded), decodedSize));
    repeater::wire::MsgpackCursor cursor(decoded, decodedSize);
    uint32_t count = 0;
    int64_t target = 0;
    TEST_ASSERT_TRUE(cursor.readArraySize(count));
    TEST_ASSERT_TRUE(cursor.readInteger(target));
    return static_cast<int>(target);
}

} // namespace

void test_a_sweep_polls_its_nine_ids_in_order_and_stores_every_reply() {
    SnapshotEngine engine;
    engine.begin(10, 200, 0); // branch 2 owns 10..18
    TEST_ASSERT_TRUE(engine.collecting());

    uint8_t poll[64];
    uint32_t now = 0;
    for(int i = 0; i < 9; ++i) {
        const size_t size = engine.nextPoll(now, poll, sizeof(poll));
        TEST_ASSERT_GREATER_THAN_UINT32(0, size);
        TEST_ASSERT_EQUAL_INT(10 + i, polledTarget(poll, size));

        // Nothing else is offered while a poll is outstanding.
        TEST_ASSERT_EQUAL_UINT32(0, engine.nextPoll(now, poll, sizeof(poll)));

        const auto reply = positionReply(10 + i);
        TEST_ASSERT_TRUE(engine.consumeInnerReply(positionInfo(10 + i), reply.data(), reply.size()));
        now += 5;
    }

    engine.service(now);
    TEST_ASSERT_FALSE(engine.collecting());
    TEST_ASSERT_EQUAL_UINT8(9, engine.storedCount());
    TEST_ASSERT_EQUAL_HEX16(0x01FF, engine.receivedMask());

    // The stored frames are the Portal's own bytes, untouched.
    for(uint8_t slot = 0; slot < 9; ++slot) {
        size_t size = 0;
        const uint8_t* stored = engine.storedFrame(slot, size);
        const auto expected = positionReply(10 + slot);
        TEST_ASSERT_EQUAL_UINT32(expected.size(), size);
        TEST_ASSERT_EQUAL_UINT8_ARRAY(expected.data(), stored, size);
    }
}

void test_only_the_outstanding_poll_is_claimed() {
    SnapshotEngine engine;
    engine.begin(1, 200, 0);

    uint8_t poll[64];
    engine.nextPoll(0, poll, sizeof(poll)); // polls ID 1

    // A different Portal's reply is relayed, not swallowed: the repeater must not
    // go blind to the rest of the branch for the length of a sweep.
    const auto other = positionReply(5);
    TEST_ASSERT_FALSE(engine.consumeInnerReply(positionInfo(5), other.data(), other.size()));

    // Nor is a log message or status report from the Portal being polled.
    InnerFrameInfo notPosition = positionInfo(1);
    notPosition.isPositionReply = false;
    TEST_ASSERT_FALSE(engine.consumeInnerReply(notPosition, other.data(), other.size()));

    // The genuine reply is claimed.
    const auto reply = positionReply(1);
    TEST_ASSERT_TRUE(engine.consumeInnerReply(positionInfo(1), reply.data(), reply.size()));

    // A duplicate arriving afterwards is no longer outstanding, so it is relayed.
    TEST_ASSERT_FALSE(engine.consumeInnerReply(positionInfo(1), reply.data(), reply.size()));
}

void test_a_silent_portal_times_out_and_the_sweep_continues() {
    SnapshotEngine engine;
    engine.begin(1, 90, 0); // 90 / 9 = 10 ms per poll

    uint8_t poll[64];
    size_t size = engine.nextPoll(0, poll, sizeof(poll));
    TEST_ASSERT_EQUAL_INT(1, polledTarget(poll, size));

    // Still within the per-poll window: no new poll yet.
    TEST_ASSERT_EQUAL_UINT32(0, engine.nextPoll(9, poll, sizeof(poll)));

    // Past it, the sweep moves on to the next ID without it.
    size = engine.nextPoll(10, poll, sizeof(poll));
    TEST_ASSERT_GREATER_THAN_UINT32(0, size);
    TEST_ASSERT_EQUAL_INT(2, polledTarget(poll, size));

    const auto reply = positionReply(2);
    TEST_ASSERT_TRUE(engine.consumeInnerReply(positionInfo(2), reply.data(), reply.size()));
    TEST_ASSERT_EQUAL_UINT8(1, engine.storedCount());
    TEST_ASSERT_EQUAL_HEX16(0x0002, engine.receivedMask()); // ID 1 missing, ID 2 present
}

void test_the_collect_window_bounds_the_sweep_even_if_nothing_answers() {
    SnapshotEngine engine;
    engine.begin(19, 60, 1000);
    uint8_t poll[64];
    TEST_ASSERT_GREATER_THAN_UINT32(0, engine.nextPoll(1000, poll, sizeof(poll)));

    TEST_ASSERT_TRUE(engine.collecting());
    engine.service(1059);
    TEST_ASSERT_TRUE(engine.collecting());

    engine.service(1060);
    TEST_ASSERT_FALSE(engine.collecting());
    TEST_ASSERT_EQUAL_UINT8(0, engine.storedCount());
    TEST_ASSERT_EQUAL_HEX16(0, engine.receivedMask());
    TEST_ASSERT_EQUAL_UINT32(60, engine.lastSweepMs());

    // And nothing further is polled once the window has closed.
    TEST_ASSERT_EQUAL_UINT32(0, engine.nextPoll(1061, poll, sizeof(poll)));
}

void test_replies_are_ignored_entirely_when_no_sweep_is_running() {
    SnapshotEngine engine;
    const auto reply = positionReply(3);
    TEST_ASSERT_FALSE(engine.consumeInnerReply(positionInfo(3), reply.data(), reply.size()));

    engine.begin(1, 100, 0);
    // Collecting, but nothing polled yet, so nothing is outstanding to claim.
    TEST_ASSERT_FALSE(engine.consumeInnerReply(positionInfo(1), reply.data(), reply.size()));
}

void test_an_oversized_reply_is_relayed_rather_than_stored() {
    SnapshotEngine engine;
    engine.begin(1, 200, 0);
    uint8_t poll[64];
    engine.nextPoll(0, poll, sizeof(poll));

    const std::vector<uint8_t> huge(repeater::SNAPSHOT_MAX_REPLY_BYTES + 1, 0x42);
    TEST_ASSERT_FALSE(engine.consumeInnerReply(positionInfo(1), huge.data(), huge.size()));
    TEST_ASSERT_EQUAL_UINT8(0, engine.storedCount());
}

void test_a_new_sweep_replaces_the_previous_one() {
    SnapshotEngine engine;
    engine.begin(1, 200, 0);
    uint8_t poll[64];
    engine.nextPoll(0, poll, sizeof(poll));
    const auto reply = positionReply(1);
    engine.consumeInnerReply(positionInfo(1), reply.data(), reply.size());
    TEST_ASSERT_EQUAL_UINT8(1, engine.storedCount());

    engine.begin(28, 200, 500);
    TEST_ASSERT_TRUE(engine.collecting());
    TEST_ASSERT_EQUAL_UINT8(0, engine.storedCount());
    TEST_ASSERT_EQUAL_HEX16(0, engine.receivedMask());
    TEST_ASSERT_EQUAL_UINT8(28, engine.rangeStart());
    const size_t size = engine.nextPoll(500, poll, sizeof(poll));
    TEST_ASSERT_EQUAL_INT(28, polledTarget(poll, size));
}

void test_a_sweep_without_a_learned_range_does_not_start() {
    SnapshotEngine engine;
    engine.begin(0, 100, 0);
    TEST_ASSERT_FALSE(engine.collecting());
    uint8_t poll[64];
    TEST_ASSERT_EQUAL_UINT32(0, engine.nextPoll(0, poll, sizeof(poll)));
}

int main(int, char**) {
    UNITY_BEGIN();
    RUN_TEST(test_a_sweep_polls_its_nine_ids_in_order_and_stores_every_reply);
    RUN_TEST(test_only_the_outstanding_poll_is_claimed);
    RUN_TEST(test_a_silent_portal_times_out_and_the_sweep_continues);
    RUN_TEST(test_the_collect_window_bounds_the_sweep_even_if_nothing_answers);
    RUN_TEST(test_replies_are_ignored_entirely_when_no_sweep_is_running);
    RUN_TEST(test_an_oversized_reply_is_relayed_rather_than_stored);
    RUN_TEST(test_a_new_sweep_replaces_the_previous_one);
    RUN_TEST(test_a_sweep_without_a_learned_range_does_not_start);
    return UNITY_END();
}
