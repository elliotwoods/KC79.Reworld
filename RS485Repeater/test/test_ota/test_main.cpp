#include <unity.h>

#include <cstdint>
#include <cstring>
#include <vector>

#include "OtaSession.h"
#include "Sha256.h"
#include "Wire.h"

using repeater::OtaBeginRequest;
using repeater::OtaResult;
using repeater::OtaSession;
using repeater::OtaState;
using repeater::OtaTarget;

namespace {

/// A slot in memory, behaving like the real one in the ways that matter: writes
/// outside the prepared image are refused, and an abort leaves it unusable.
class FakeTarget : public OtaTarget {
public:
    bool beginImage(uint32_t imageSize) override {
        if(failBegin) return false;
        storage.assign(imageSize, 0xFF);
        prepared = true;
        committed = false;
        beginCalls++;
        return true;
    }

    bool writeAt(uint32_t offset, const uint8_t* data, size_t size) override {
        if(!prepared || failWrite) return false;
        if(offset > storage.size() || size > storage.size() - offset) return false;
        std::memcpy(storage.data() + offset, data, size);
        writeCalls++;
        return true;
    }

    bool readAt(uint32_t offset, uint8_t* data, size_t size) override {
        if(!prepared) return false;
        if(offset > storage.size() || size > storage.size() - offset) return false;
        std::memcpy(data, storage.data() + offset, size);
        return true;
    }

    bool commit() override {
        if(!prepared || failCommit) return false;
        committed = true;
        return true;
    }

    void abortImage() override {
        prepared = false;
        abortCalls++;
    }

    std::vector<uint8_t> storage;
    bool prepared = false;
    bool committed = false;
    bool failBegin = false;
    bool failWrite = false;
    bool failCommit = false;
    int beginCalls = 0;
    int writeCalls = 0;
    int abortCalls = 0;
};

std::vector<uint8_t> makeImage(size_t size) {
    std::vector<uint8_t> image(size);
    for(size_t i = 0; i < size; ++i) image[i] = static_cast<uint8_t>((i * 31 + 7) & 0xFF);
    return image;
}

OtaBeginRequest requestFor(const std::vector<uint8_t>& image, uint32_t chunkBytes, uint8_t session) {
    OtaBeginRequest request;
    request.imageSize = static_cast<uint32_t>(image.size());
    request.chunkBytes = chunkBytes;
    request.session = session;
    repeater::Sha256::hash(image.data(), image.size(), request.sha256);
    return request;
}

/// Sends chunk `index`, computing its CRC the way the host would.
OtaResult sendChunk(OtaSession& session, uint8_t sessionId, const std::vector<uint8_t>& image,
    uint32_t chunkBytes, uint32_t index, uint32_t nowMs) {
    const uint32_t offset = index * chunkBytes;
    const uint32_t size = (offset + chunkBytes > image.size())
        ? static_cast<uint32_t>(image.size() - offset)
        : chunkBytes;
    const uint16_t crc = repeater::wire::crc16CcittFalse(image.data() + offset, size);
    return session.writeChunk(sessionId, index, image.data() + offset, size, crc, nowMs);
}

} // namespace

void test_sha256_matches_known_vectors() {
    uint8_t digest[32];

    repeater::Sha256::hash(reinterpret_cast<const uint8_t*>(""), 0, digest);
    const uint8_t empty[32] = {
        0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8,
        0x99, 0x6f, 0xb9, 0x24, 0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c,
        0xa4, 0x95, 0x99, 0x1b, 0x78, 0x52, 0xb8, 0x55,
    };
    TEST_ASSERT_EQUAL_UINT8_ARRAY(empty, digest, 32);

    repeater::Sha256::hash(reinterpret_cast<const uint8_t*>("abc"), 3, digest);
    const uint8_t abc[32] = {
        0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde,
        0x5d, 0xae, 0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c,
        0xb4, 0x10, 0xff, 0x61, 0xf2, 0x00, 0x15, 0xad,
    };
    TEST_ASSERT_EQUAL_UINT8_ARRAY(abc, digest, 32);

    // Spans several blocks and exercises the length padding.
    const char* long56 = "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
    repeater::Sha256::hash(reinterpret_cast<const uint8_t*>(long56), 56, digest);
    const uint8_t expected[32] = {
        0x24, 0x8d, 0x6a, 0x61, 0xd2, 0x06, 0x38, 0xb8, 0xe5, 0xc0, 0x26, 0x93,
        0x0c, 0x3e, 0x60, 0x39, 0xa3, 0x3c, 0xe4, 0x59, 0x64, 0xff, 0x21, 0x67,
        0xf6, 0xec, 0xed, 0xd4, 0x19, 0xdb, 0x06, 0xc1,
    };
    TEST_ASSERT_EQUAL_UINT8_ARRAY(expected, digest, 32);
}

void test_in_order_transfer_verifies_and_commits() {
    FakeTarget target;
    OtaSession session(target);
    const auto image = makeImage(4000);
    const auto request = requestFor(image, 512, 7);

    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(OtaResult::Ok), static_cast<uint8_t>(session.begin(request, 0)));
    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(OtaState::Receiving), static_cast<uint8_t>(session.state()));
    TEST_ASSERT_EQUAL_UINT32(8, session.chunkCount()); // 4000 / 512 rounded up

    for(uint32_t i = 0; i < session.chunkCount(); ++i) {
        TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(OtaResult::Ok),
            static_cast<uint8_t>(sendChunk(session, 7, image, 512, i, 10 * i)));
    }
    TEST_ASSERT_EQUAL_UINT32(8, session.receivedChunks());
    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(OtaResult::Ok), static_cast<uint8_t>(session.finish(100)));
    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(OtaState::Ready), static_cast<uint8_t>(session.state()));
    TEST_ASSERT_TRUE(target.committed);
    TEST_ASSERT_EQUAL_UINT8_ARRAY(image.data(), target.storage.data(), image.size());
}

void test_chunks_may_arrive_in_any_order() {
    FakeTarget target;
    OtaSession session(target);
    const auto image = makeImage(4000);
    session.begin(requestFor(image, 512, 1), 0);

    const uint32_t order[] = {5, 0, 7, 2, 1, 6, 3, 4};
    for(uint32_t index : order) {
        TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(OtaResult::Ok),
            static_cast<uint8_t>(sendChunk(session, 1, image, 512, index, 1)));
    }
    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(OtaResult::Ok), static_cast<uint8_t>(session.finish(2)));
    TEST_ASSERT_EQUAL_UINT8_ARRAY(image.data(), target.storage.data(), image.size());
}

void test_missing_chunks_are_reported_by_the_bitmap_and_repairable() {
    FakeTarget target;
    OtaSession session(target);
    const auto image = makeImage(4000);
    session.begin(requestFor(image, 512, 2), 0);

    // Everything except 1 and 6, the pattern a lossy broadcast pass produces.
    for(uint32_t i = 0; i < 8; ++i) {
        if(i == 1 || i == 6) continue;
        sendChunk(session, 2, image, 512, i, 1);
    }
    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(OtaResult::Incomplete), static_cast<uint8_t>(session.finish(2)));
    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(OtaState::Receiving), static_cast<uint8_t>(session.state()));

    TEST_ASSERT_EQUAL_UINT32(1, session.bitmapBytes());
    const uint8_t map = session.bitmap()[0];
    TEST_ASSERT_EQUAL_HEX8(0b10111101, map); // bits 1 and 6 clear

    sendChunk(session, 2, image, 512, 1, 3);
    sendChunk(session, 2, image, 512, 6, 3);
    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(OtaResult::Ok), static_cast<uint8_t>(session.finish(4)));
    TEST_ASSERT_EQUAL_UINT8_ARRAY(image.data(), target.storage.data(), image.size());
}

void test_a_chunk_without_a_session_is_refused_rather_than_written() {
    FakeTarget target;
    OtaSession session(target);
    const auto image = makeImage(1024);
    // No begin: on real hardware an unguarded write here asserts inside IDF and
    // reboots the repeater.
    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(OtaResult::NoSession),
        static_cast<uint8_t>(sendChunk(session, 1, image, 512, 0, 0)));
    TEST_ASSERT_EQUAL_INT(0, target.writeCalls);
    TEST_ASSERT_EQUAL_INT(0, target.beginCalls);
}

void test_a_chunk_from_a_different_session_is_refused() {
    FakeTarget target;
    OtaSession session(target);
    const auto image = makeImage(2048);
    session.begin(requestFor(image, 512, 9), 0);

    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(OtaResult::WrongSession),
        static_cast<uint8_t>(sendChunk(session, 10, image, 512, 0, 1)));
    TEST_ASSERT_EQUAL_UINT32(0, session.receivedChunks());
    TEST_ASSERT_EQUAL_INT(0, target.writeCalls);
}

void test_a_corrupt_chunk_is_left_unmarked_so_repair_collects_it() {
    FakeTarget target;
    OtaSession session(target);
    const auto image = makeImage(1024);
    session.begin(requestFor(image, 512, 3), 0);

    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(OtaResult::BadCrc),
        static_cast<uint8_t>(session.writeChunk(3, 0, image.data(), 512, 0x0000, 1)));
    TEST_ASSERT_EQUAL_UINT32(0, session.receivedChunks());
    TEST_ASSERT_EQUAL_INT(0, target.writeCalls);

    // The genuine chunk still lands.
    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(OtaResult::Ok),
        static_cast<uint8_t>(sendChunk(session, 3, image, 512, 0, 2)));
    TEST_ASSERT_EQUAL_UINT32(1, session.receivedChunks());
}

void test_a_wrong_image_fails_verification_and_is_never_committed() {
    FakeTarget target;
    OtaSession session(target);
    auto image = makeImage(2048);
    const auto request = requestFor(image, 512, 4);
    session.begin(request, 0);

    // Every chunk arrives with a valid CRC, but one carries different content —
    // exactly what a host that sent the wrong file would produce.
    image[600] ^= 0xFF;
    for(uint32_t i = 0; i < 4; ++i) sendChunk(session, 4, image, 512, i, 1);

    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(OtaResult::VerifyFailed), static_cast<uint8_t>(session.finish(2)));
    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(OtaState::Failed), static_cast<uint8_t>(session.state()));
    TEST_ASSERT_FALSE(target.committed);
    TEST_ASSERT_EQUAL_INT(1, target.abortCalls);
}

void test_an_abandoned_session_times_out_and_releases_the_bridge() {
    FakeTarget target;
    OtaSession session(target);
    const auto image = makeImage(2048);
    session.begin(requestFor(image, 512, 5), 1000);
    sendChunk(session, 5, image, 512, 0, 1500);
    TEST_ASSERT_TRUE(session.busy());

    session.service(1500 + repeater::OTA_INACTIVITY_TIMEOUT_MS - 1);
    TEST_ASSERT_TRUE(session.busy());

    session.service(1500 + repeater::OTA_INACTIVITY_TIMEOUT_MS);
    TEST_ASSERT_FALSE(session.busy());
    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(OtaState::Idle), static_cast<uint8_t>(session.state()));
    TEST_ASSERT_EQUAL_INT(1, target.abortCalls);
}

void test_a_second_begin_abandons_the_first_rather_than_leaking_it() {
    FakeTarget target;
    OtaSession session(target);
    const auto image = makeImage(2048);
    session.begin(requestFor(image, 512, 1), 0);
    sendChunk(session, 1, image, 512, 0, 1);

    session.begin(requestFor(image, 512, 2), 10);
    TEST_ASSERT_EQUAL_INT(1, target.abortCalls);
    TEST_ASSERT_EQUAL_INT(2, target.beginCalls);
    TEST_ASSERT_EQUAL_UINT8(2, session.session());
    TEST_ASSERT_EQUAL_UINT32(0, session.receivedChunks());
}

void test_malformed_begin_requests_are_refused() {
    FakeTarget target;
    OtaSession session(target);
    OtaBeginRequest request;

    request.imageSize = 0;
    request.chunkBytes = 512;
    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(OtaResult::BadRequest), static_cast<uint8_t>(session.begin(request, 0)));

    request.imageSize = 1024;
    request.chunkBytes = 0;
    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(OtaResult::BadRequest), static_cast<uint8_t>(session.begin(request, 0)));

    request.chunkBytes = repeater::OTA_MAX_CHUNK_BYTES + 1;
    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(OtaResult::BadRequest), static_cast<uint8_t>(session.begin(request, 0)));

    // More chunks than the bitmap can track.
    request.chunkBytes = 1;
    request.imageSize = repeater::OTA_MAX_CHUNKS + 1;
    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(OtaResult::BadRequest), static_cast<uint8_t>(session.begin(request, 0)));

    TEST_ASSERT_EQUAL_INT(0, target.beginCalls);
}

void test_an_erase_failure_is_reported_and_leaves_no_session() {
    FakeTarget target;
    target.failBegin = true;
    OtaSession session(target);
    const auto image = makeImage(1024);
    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(OtaResult::EraseFailed),
        static_cast<uint8_t>(session.begin(requestFor(image, 512, 1), 0)));
    TEST_ASSERT_FALSE(session.busy());
    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(OtaResult::NoSession),
        static_cast<uint8_t>(sendChunk(session, 1, image, 512, 0, 1)));
}

void test_a_short_final_chunk_is_required_to_be_exactly_the_remainder() {
    FakeTarget target;
    OtaSession session(target);
    const auto image = makeImage(1000); // 512 + 488
    session.begin(requestFor(image, 512, 6), 0);
    TEST_ASSERT_EQUAL_UINT32(2, session.chunkCount());

    // A full-size final chunk would run past the image.
    const uint16_t crc = repeater::wire::crc16CcittFalse(image.data() + 512, 488);
    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(OtaResult::BadIndex),
        static_cast<uint8_t>(session.writeChunk(6, 1, image.data() + 512, 512, crc, 1)));

    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(OtaResult::Ok),
        static_cast<uint8_t>(session.writeChunk(6, 1, image.data() + 512, 488, crc, 1)));

    // And an index past the end is refused before the data is touched at all.
    const uint16_t tailCrc = repeater::wire::crc16CcittFalse(image.data(), 512);
    TEST_ASSERT_EQUAL_UINT8(static_cast<uint8_t>(OtaResult::BadIndex),
        static_cast<uint8_t>(session.writeChunk(6, 2, image.data(), 512, tailCrc, 1)));
}

int main(int, char**) {
    UNITY_BEGIN();
    RUN_TEST(test_sha256_matches_known_vectors);
    RUN_TEST(test_in_order_transfer_verifies_and_commits);
    RUN_TEST(test_chunks_may_arrive_in_any_order);
    RUN_TEST(test_missing_chunks_are_reported_by_the_bitmap_and_repairable);
    RUN_TEST(test_a_chunk_without_a_session_is_refused_rather_than_written);
    RUN_TEST(test_a_chunk_from_a_different_session_is_refused);
    RUN_TEST(test_a_corrupt_chunk_is_left_unmarked_so_repair_collects_it);
    RUN_TEST(test_a_wrong_image_fails_verification_and_is_never_committed);
    RUN_TEST(test_an_abandoned_session_times_out_and_releases_the_bridge);
    RUN_TEST(test_a_second_begin_abandons_the_first_rather_than_leaking_it);
    RUN_TEST(test_malformed_begin_requests_are_refused);
    RUN_TEST(test_an_erase_failure_is_reported_and_leaves_no_session);
    RUN_TEST(test_a_short_final_chunk_is_required_to_be_exactly_the_remainder);
    return UNITY_END();
}
