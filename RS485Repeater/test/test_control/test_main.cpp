#include <unity.h>

#include <cstdint>
#include <cstring>
#include <vector>

#include "ControlPlane.h"
#include "Wire.h"

using repeater::ControlPlane;
using repeater::ControlRequest;
using repeater::ControlVerb;
using repeater::REPEATER_ALL;

namespace {

const uint8_t MAC[6] = {0xF8, 0x5B, 0x1B, 0xED, 0x8D, 0xA4};
const uint8_t OTHER_MAC[6] = {0xF8, 0x5B, 0x1B, 0xF4, 0x18, 0xEC};

std::vector<uint8_t> frame(const std::vector<uint8_t>& raw) {
    std::vector<uint8_t> out(raw.size() + raw.size() / 254 + 2);
    const size_t size = repeater::wire::cobsEncodeFrame(raw.data(), raw.size(), out.data(), out.size());
    out.resize(size);
    return out;
}

/// `[0, 0, {"rq": {"a": <address>, "q": "<verb>"}}]`
std::vector<uint8_t> request(int address, const char* verb) {
    uint8_t buffer[256];
    repeater::wire::MsgpackWriter w(buffer, sizeof(buffer));
    w.arrayHeader(3);
    w.integer(0);
    w.integer(0);
    w.mapHeader(1);
    w.key("rq");
    w.mapHeader(2);
    w.key("a");
    w.integer(address);
    w.key("q");
    w.string(verb);
    TEST_ASSERT_TRUE(w.ok());
    return frame(std::vector<uint8_t>(w.data(), w.data() + w.size()));
}

/// The same, addressed by MAC rather than index.
std::vector<uint8_t> requestByMac(const uint8_t mac[6], const char* verb) {
    uint8_t buffer[256];
    repeater::wire::MsgpackWriter w(buffer, sizeof(buffer));
    w.arrayHeader(3);
    w.integer(0);
    w.integer(0);
    w.mapHeader(1);
    w.key("rq");
    w.mapHeader(2);
    w.key("a");
    w.binary(mac, 6);
    w.key("q");
    w.string(verb);
    TEST_ASSERT_TRUE(w.ok());
    return frame(std::vector<uint8_t>(w.data(), w.data() + w.size()));
}

ControlPlane provisioned(int8_t index = 3) {
    ControlPlane plane;
    plane.setIdentity(index, MAC);
    return plane;
}

} // namespace

void test_repeater_addresses_round_trip() {
    for(int8_t index = 1; index <= 6; ++index) {
        const int8_t address = repeater::repeaterAddress(index);
        TEST_ASSERT_EQUAL_INT(-(2 + index), address);
        TEST_ASSERT_EQUAL_INT(index, repeater::repeaterIndexFromAddress(address));
    }
    // Nothing outside the repeater block decodes as a repeater.
    TEST_ASSERT_EQUAL_INT(0, repeater::repeaterAddress(0));
    TEST_ASSERT_EQUAL_INT(0, repeater::repeaterAddress(7));
    TEST_ASSERT_EQUAL_INT(0, repeater::repeaterIndexFromAddress(0));
    TEST_ASSERT_EQUAL_INT(0, repeater::repeaterIndexFromAddress(-1)); // broadcast
    TEST_ASSERT_EQUAL_INT(0, repeater::repeaterIndexFromAddress(REPEATER_ALL));
    TEST_ASSERT_EQUAL_INT(0, repeater::repeaterIndexFromAddress(-9));
    TEST_ASSERT_EQUAL_INT(0, repeater::repeaterIndexFromAddress(5)); // a Portal
}

void test_unicast_request_for_this_repeater_is_claimed() {
    ControlPlane plane = provisioned(3);
    const auto raw = request(repeater::repeaterAddress(3), "status");
    const ControlRequest parsed = plane.parse(raw.data(), raw.size());
    TEST_ASSERT_TRUE(parsed.valid);
    TEST_ASSERT_TRUE(parsed.addressedToUs);
    TEST_ASSERT_FALSE(parsed.broadcast);
    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(ControlVerb::Status), static_cast<uint8_t>(parsed.verb));
}

void test_unicast_request_for_another_repeater_is_recognised_but_not_ours() {
    ControlPlane plane = provisioned(3);
    const auto raw = request(repeater::repeaterAddress(5), "status");
    const ControlRequest parsed = plane.parse(raw.data(), raw.size());
    TEST_ASSERT_TRUE(parsed.valid); // still a control frame, so it is never forwarded
    TEST_ASSERT_FALSE(parsed.addressedToUs);
}

void test_reply_bearing_verbs_are_refused_on_the_broadcast_address() {
    ControlPlane plane = provisioned(3);
    // Six repeaters answering a broadcast `status` at once would collide.
    const auto status = request(REPEATER_ALL, "status");
    ControlRequest parsed = plane.parse(status.data(), status.size());
    TEST_ASSERT_TRUE(parsed.valid);
    TEST_ASSERT_TRUE(parsed.broadcast);
    TEST_ASSERT_FALSE(parsed.addressedToUs);

    // A verb that solicits no reply is fine broadcast.
    const auto snap = request(REPEATER_ALL, "snap-start");
    parsed = plane.parse(snap.data(), snap.size());
    TEST_ASSERT_TRUE(parsed.addressedToUs);
    TEST_ASSERT_TRUE(parsed.broadcast);
}

void test_mac_addressing_reaches_a_unit_with_the_wrong_or_no_index() {
    ControlPlane plane;
    plane.setIdentity(0, MAC); // unprovisioned: no index at all

    const auto byIndex = request(repeater::repeaterAddress(1), "status");
    ControlRequest parsed = plane.parse(byIndex.data(), byIndex.size());
    TEST_ASSERT_TRUE(parsed.valid);
    TEST_ASSERT_FALSE(parsed.addressedToUs);

    const auto byMac = requestByMac(MAC, "status");
    parsed = plane.parse(byMac.data(), byMac.size());
    TEST_ASSERT_TRUE(parsed.addressedToUs);

    const auto byOtherMac = requestByMac(OTHER_MAC, "status");
    parsed = plane.parse(byOtherMac.data(), byOtherMac.size());
    TEST_ASSERT_TRUE(parsed.valid);
    TEST_ASSERT_FALSE(parsed.addressedToUs);
}

void test_ordinary_traffic_is_not_mistaken_for_control_traffic() {
    ControlPlane plane = provisioned(3);

    // A Portal reply crossing the outer bus: same envelope target, different body.
    uint8_t buffer[64];
    repeater::wire::MsgpackWriter w(buffer, sizeof(buffer));
    w.arrayHeader(3);
    w.integer(0);
    w.integer(7);
    w.mapHeader(1);
    w.key("p");
    w.arrayHeader(4);
    w.integer(1);
    w.integer(2);
    w.integer(3);
    w.integer(4);
    const auto reply = frame(std::vector<uint8_t>(w.data(), w.data() + w.size()));
    ControlRequest parsed = plane.parse(reply.data(), reply.size());
    TEST_ASSERT_FALSE(parsed.valid);

    // A broadcast keyframe is not target 0 at all.
    repeater::wire::MsgpackWriter k(buffer, sizeof(buffer));
    k.arrayHeader(3);
    k.integer(-1);
    k.integer(0);
    k.mapHeader(1);
    k.key("keyframe");
    k.nil();
    const auto keyframe = frame(std::vector<uint8_t>(k.data(), k.data() + k.size()));
    parsed = plane.parse(keyframe.data(), keyframe.size());
    TEST_ASSERT_FALSE(parsed.valid);

    // The Portal firmware-update announce, which must stay untouched.
    const std::vector<uint8_t> announce{0x93, 0xD0, 0xFF, 0xD0, 0x00, 0xA2, 'F', 'W'};
    const auto fw = frame(announce);
    parsed = plane.parse(fw.data(), fw.size());
    TEST_ASSERT_FALSE(parsed.valid);
}

void test_a_request_without_an_address_is_rejected() {
    ControlPlane plane = provisioned(3);
    uint8_t buffer[64];
    repeater::wire::MsgpackWriter w(buffer, sizeof(buffer));
    w.arrayHeader(3);
    w.integer(0);
    w.integer(0);
    w.mapHeader(1);
    w.key("rq");
    w.mapHeader(1);
    w.key("q");
    w.string("status");
    const auto raw = frame(std::vector<uint8_t>(w.data(), w.data() + w.size()));
    const ControlRequest parsed = plane.parse(raw.data(), raw.size());
    TEST_ASSERT_FALSE(parsed.valid);
}

void test_payload_is_exposed_as_raw_msgpack() {
    ControlPlane plane = provisioned(2);
    uint8_t buffer[64];
    repeater::wire::MsgpackWriter w(buffer, sizeof(buffer));
    w.arrayHeader(3);
    w.integer(0);
    w.integer(0);
    w.mapHeader(1);
    w.key("rq");
    w.mapHeader(3);
    w.key("a");
    w.integer(repeater::repeaterAddress(2));
    w.key("q");
    w.string("set-index");
    w.key("v");
    w.integer(4);
    const auto raw = frame(std::vector<uint8_t>(w.data(), w.data() + w.size()));

    const ControlRequest parsed = plane.parse(raw.data(), raw.size());
    TEST_ASSERT_TRUE(parsed.addressedToUs);
    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(ControlVerb::SetIndex), static_cast<uint8_t>(parsed.verb));
    TEST_ASSERT_EQUAL_UINT32(1, parsed.payloadSize);

    repeater::wire::MsgpackCursor cursor(parsed.payload, parsed.payloadSize);
    int64_t value = 0;
    TEST_ASSERT_TRUE(cursor.readInteger(value));
    TEST_ASSERT_EQUAL_INT64(4, value);
}

void test_reply_is_a_well_formed_envelope_sourced_from_this_repeater() {
    ControlPlane plane = provisioned(4);
    auto& out = plane.beginReply(ControlVerb::Status, true, true);
    out.mapHeader(1);
    out.key("proto");
    out.uinteger(repeater::CONTROL_PROTO_VERSION);

    uint8_t framed[512];
    const size_t size = plane.finishReply(framed, sizeof(framed));
    TEST_ASSERT_GREATER_THAN_UINT32(0, size);
    TEST_ASSERT_EQUAL_UINT8(0, framed[size - 1]);

    uint8_t decoded[512];
    size_t decodedSize = 0;
    TEST_ASSERT_TRUE(repeater::wire::cobsDecode(framed, size - 1, decoded, sizeof(decoded), decodedSize));

    repeater::wire::MsgpackCursor cursor(decoded, decodedSize);
    uint32_t count = 0;
    int64_t target = 0;
    int64_t source = 0;
    TEST_ASSERT_TRUE(cursor.readArraySize(count));
    TEST_ASSERT_EQUAL_UINT32(3, count);
    TEST_ASSERT_TRUE(cursor.readInteger(target));
    TEST_ASSERT_TRUE(cursor.readInteger(source));
    TEST_ASSERT_EQUAL_INT64(0, target);
    TEST_ASSERT_EQUAL_INT64(repeater::repeaterAddress(4), source);

    TEST_ASSERT_TRUE(cursor.readMapSize(count));
    TEST_ASSERT_EQUAL_UINT32(1, count);
    const uint8_t* key = nullptr;
    uint32_t keyLength = 0;
    TEST_ASSERT_TRUE(cursor.readString(key, keyLength));
    TEST_ASSERT_TRUE(repeater::wire::stringEquals(key, keyLength, "rr"));
    TEST_ASSERT_TRUE(cursor.readMapSize(count));
    TEST_ASSERT_EQUAL_UINT32(4, count); // a, q, ok, v
}

void test_unprovisioned_repeater_still_answers_a_mac_addressed_request() {
    ControlPlane plane;
    plane.setIdentity(0, MAC);
    plane.beginReply(ControlVerb::Status, true, false);

    uint8_t framed[256];
    const size_t size = plane.finishReply(framed, sizeof(framed));
    TEST_ASSERT_GREATER_THAN_UINT32(0, size);

    uint8_t decoded[256];
    size_t decodedSize = 0;
    TEST_ASSERT_TRUE(repeater::wire::cobsDecode(framed, size - 1, decoded, sizeof(decoded), decodedSize));
    repeater::wire::MsgpackCursor cursor(decoded, decodedSize);
    uint32_t count = 0;
    int64_t target = 0;
    int64_t source = 0;
    cursor.readArraySize(count);
    cursor.readInteger(target);
    cursor.readInteger(source);
    TEST_ASSERT_EQUAL_INT64(REPEATER_ALL, source);
}

void test_crc16_matches_the_pinned_definition() {
    const char* check = "123456789";
    TEST_ASSERT_EQUAL_HEX16(0x29B1,
        repeater::wire::crc16CcittFalse(reinterpret_cast<const uint8_t*>(check), 9));
}

void test_cobs_round_trips_including_zero_runs() {
    const std::vector<std::vector<uint8_t>> cases{
        {0x11, 0x22, 0x00, 0x33},
        {0x00},
        {0x00, 0x00, 0x00},
        {0x01},
        std::vector<uint8_t>(600, 0x7F),
        std::vector<uint8_t>(300, 0x00),
    };
    for(const auto& raw : cases) {
        std::vector<uint8_t> encoded(raw.size() + raw.size() / 254 + 2);
        const size_t size = repeater::wire::cobsEncodeFrame(raw.data(), raw.size(), encoded.data(), encoded.size());
        TEST_ASSERT_GREATER_THAN_UINT32(0, size);
        TEST_ASSERT_EQUAL_UINT8(0, encoded[size - 1]);

        std::vector<uint8_t> decoded(raw.size() + 8);
        size_t decodedSize = 0;
        TEST_ASSERT_TRUE(repeater::wire::cobsDecode(encoded.data(), size - 1, decoded.data(), decoded.size(), decodedSize));
        TEST_ASSERT_EQUAL_UINT32(raw.size(), decodedSize);
        if(!raw.empty()) TEST_ASSERT_EQUAL_UINT8_ARRAY(raw.data(), decoded.data(), raw.size());
    }
}

void test_msgpack_integer_encoding_matches_the_host_encoder() {
    // Mirrors `dump_int` in RouterRS/crates/router-proto/src/value.rs.
    struct Case { int64_t value; std::vector<uint8_t> bytes; };
    const std::vector<Case> cases{
        {0, {0x00}},
        {127, {0x7F}},
        {128, {0xCC, 0x80}},
        {255, {0xCC, 0xFF}},
        {256, {0xCD, 0x01, 0x00}},
        {65536, {0xCE, 0x00, 0x01, 0x00, 0x00}},
        {-1, {0xFF}},
        {-2, {0xFE}},   // REPEATER_ALL
        {-8, {0xF8}},   // repeater 6
        {-32, {0xE0}},
        {-33, {0xD0, 0xDF}},
        {-128, {0xD0, 0x80}},
        {-129, {0xD1, 0xFF, 0x7F}},
    };
    for(const auto& c : cases) {
        uint8_t buffer[16];
        repeater::wire::MsgpackWriter w(buffer, sizeof(buffer));
        w.integer(c.value);
        TEST_ASSERT_TRUE(w.ok());
        TEST_ASSERT_EQUAL_UINT32(c.bytes.size(), w.size());
        TEST_ASSERT_EQUAL_UINT8_ARRAY(c.bytes.data(), w.data(), c.bytes.size());

        repeater::wire::MsgpackCursor cursor(buffer, w.size());
        int64_t read = 0;
        TEST_ASSERT_TRUE(cursor.readInteger(read));
        TEST_ASSERT_EQUAL_INT64(c.value, read);
    }
}

void test_writer_latches_overflow_rather_than_truncating_silently() {
    uint8_t buffer[4];
    repeater::wire::MsgpackWriter w(buffer, sizeof(buffer));
    w.string("this string is far too long for the buffer");
    TEST_ASSERT_FALSE(w.ok());
}

int main(int, char**) {
    UNITY_BEGIN();
    RUN_TEST(test_repeater_addresses_round_trip);
    RUN_TEST(test_unicast_request_for_this_repeater_is_claimed);
    RUN_TEST(test_unicast_request_for_another_repeater_is_recognised_but_not_ours);
    RUN_TEST(test_reply_bearing_verbs_are_refused_on_the_broadcast_address);
    RUN_TEST(test_mac_addressing_reaches_a_unit_with_the_wrong_or_no_index);
    RUN_TEST(test_ordinary_traffic_is_not_mistaken_for_control_traffic);
    RUN_TEST(test_a_request_without_an_address_is_rejected);
    RUN_TEST(test_payload_is_exposed_as_raw_msgpack);
    RUN_TEST(test_reply_is_a_well_formed_envelope_sourced_from_this_repeater);
    RUN_TEST(test_unprovisioned_repeater_still_answers_a_mac_addressed_request);
    RUN_TEST(test_crc16_matches_the_pinned_definition);
    RUN_TEST(test_cobs_round_trips_including_zero_runs);
    RUN_TEST(test_msgpack_integer_encoding_matches_the_host_encoder);
    RUN_TEST(test_writer_latches_overflow_rather_than_truncating_silently);
    return UNITY_END();
}
