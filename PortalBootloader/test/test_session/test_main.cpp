// The upload session: erasing, writing, and the two things that make writing hard.
//
// Out-of-order and duplicate frames are the entire reason this is not a `memcpy`. The legacy host
// sends every frame twice by default, and a repair pass resends an arbitrary subset; meanwhile the
// flash controller refuses to program a double-word that is not erased, and the fake here refuses
// too. So "write this payload at this offset" has to be idempotent at 8-byte resolution or an
// ordinary update destroys itself on its second frame.

#include <unity.h>

#include "bl/session.hpp"
#include "bl/image.hpp"
#include "fake_hw.hpp"

#include <string.h>

using namespace bl;

namespace {
	Session session;

	/// Run an armed erase to completion, as the main loop would.
	void finishErase(Session & target)
	{
		uint32_t guard = 0;
		while(!target.eraseStep() && guard++ < 200) {
		}
	}

	void fillPattern(uint8_t * out, uint32_t length, uint8_t seed)
	{
		for(uint32_t index = 0; index < length; index++) {
			out[index] = (uint8_t) (seed + index);
		}
	}
}

void setUp()
{
	bltest::reset();
	session = Session();
}

void tearDown() {}

// ---- Erase -------------------------------------------------------------------------------------

void test_erase_covers_the_whole_bank_one_page_at_a_time()
{
	session.beginErase(config::appBase);
	TEST_ASSERT_TRUE(session.isErasing());

	uint32_t steps = 0;
	while(!session.eraseStep()) {
		steps++;
		TEST_ASSERT_LESS_THAN_UINT32(200, steps);
	}

	// One page per call, 53 pages, and the caller got to run between every one of them. That is
	// what keeps the receive ring from overflowing across a 1.2 s erase.
	TEST_ASSERT_EQUAL_UINT32(config::appPageCount, bltest::erasedPages());
	TEST_ASSERT_FALSE(session.isErasing());
	TEST_ASSERT_EQUAL_UINT32(config::appPageCount, session.erasePages());
}

void test_erase_never_reaches_the_durable_pages()
{
	// Provisioning identity and both settings journals, marked so their survival is visible.
	const uint8_t sentinel[] = {'K', 'C', 'P', 'R', 'V', '0', '0', '1'};
	bltest::preload(PORTAL_PERSIST_IDENTITY, sentinel, sizeof(sentinel));
	bltest::preload(PORTAL_PERSIST_SETTINGS_A, sentinel, sizeof(sentinel));
	bltest::preload(PORTAL_PERSIST_SETTINGS_B, sentinel, sizeof(sentinel));

	session.beginErase(config::appBase);
	finishErase(session);

	// This is the defect the whole rewrite exists to fix. The fielded v4 bootloader computed
	// `NbPages = (FLASH_SIZE - appOffset) / FLASH_PAGE_SIZE`, which is 52 pages -- three more than
	// the application bank -- so every field update erased the board's serial number and settings.
	TEST_ASSERT_EQUAL_UINT8_ARRAY(sentinel, bltest::flashAt(PORTAL_PERSIST_IDENTITY),
		sizeof(sentinel));
	TEST_ASSERT_EQUAL_UINT8_ARRAY(sentinel, bltest::flashAt(PORTAL_PERSIST_SETTINGS_A),
		sizeof(sentinel));
	TEST_ASSERT_EQUAL_UINT8_ARRAY(sentinel, bltest::flashAt(PORTAL_PERSIST_SETTINGS_B),
		sizeof(sentinel));
}

void test_erase_never_reaches_the_bootloader_itself()
{
	const uint8_t sentinel[] = {0xDE, 0xAD, 0xBE, 0xEF};
	bltest::preload(PORTAL_FLASH_BASE, sentinel, sizeof(sentinel));
	bltest::preload(PORTAL_FLASH_BASE + 0x3FF0, sentinel, sizeof(sentinel));

	session.beginErase(config::appBase);
	finishErase(session);

	TEST_ASSERT_EQUAL_UINT8_ARRAY(sentinel, bltest::flashAt(PORTAL_FLASH_BASE), sizeof(sentinel));
	TEST_ASSERT_EQUAL_UINT8_ARRAY(sentinel, bltest::flashAt(PORTAL_FLASH_BASE + 0x3FF0),
		sizeof(sentinel));
}

void test_a_legacy_erase_still_clears_the_new_bank()
{
	// A legacy host's `ER` writes at 0x08006000, but the 8 kB below it must still be cleared:
	// `decideRun` only falls back to a legacy-base application when the new bank is blank, so
	// leaving a stale new-base image there would let it shadow the one just uploaded.
	const uint8_t stale[] = {0x11, 0x22, 0x33, 0x44};
	bltest::preload(config::appBase, stale, sizeof(stale));

	session.beginErase(config::appBaseLegacy);
	finishErase(session);

	TEST_ASSERT_TRUE(regionBlank(config::appBase, 0x2000));
	TEST_ASSERT_EQUAL_UINT32(config::appBaseLegacy, session.base());
}

void test_a_page_that_will_not_erase_stops_rather_than_looping()
{
	bltest::failEraseOf(config::appFirstPage + 3);
	session.beginErase(config::appBase);
	finishErase(session);

	// Stopping leaves the fault visible in `status`. Retrying the same page forever would look
	// from outside exactly like a boot loop, which is a much harder thing to diagnose.
	TEST_ASSERT_FALSE(session.isErasing());
	TEST_ASSERT_LESS_THAN_UINT32(config::appPageCount, bltest::erasedPages());
}

// ---- Writing -----------------------------------------------------------------------------------

void test_a_simple_sequential_upload_lands_byte_for_byte()
{
	session.beginErase(config::appBase);
	finishErase(session);

	uint8_t payload[128];
	fillPattern(payload, sizeof(payload), 0x40);

	for(uint32_t offset = 0; offset < 512; offset += sizeof(payload)) {
		TEST_ASSERT_EQUAL(Error::None, session.write(offset, payload, sizeof(payload)));
	}

	for(uint32_t offset = 0; offset < 512; offset += sizeof(payload)) {
		TEST_ASSERT_EQUAL_UINT8_ARRAY(payload, bltest::flashAt(config::appBase + offset),
			sizeof(payload));
	}
	TEST_ASSERT_EQUAL_UINT32(512, session.highWater());
	TEST_ASSERT_EQUAL_UINT32(512, session.received());
}

void test_frames_may_arrive_in_any_order()
{
	session.beginErase(config::appBase);
	finishErase(session);

	uint8_t chunks[8][64];
	for(uint8_t index = 0; index < 8; index++) {
		fillPattern(chunks[index], sizeof(chunks[index]), (uint8_t) (index * 17));
	}

	// Deliberately shuffled. The fielded bootloader required strictly increasing offsets and
	// rejected everything after the first gap, which is why one lost frame silently ended an
	// upload -- the host had no way to be told, and kept sending.
	const uint8_t order[8] = {5, 0, 7, 2, 1, 6, 3, 4};
	for(uint8_t step = 0; step < 8; step++) {
		const uint8_t index = order[step];
		TEST_ASSERT_EQUAL(Error::None,
			session.write(index * 64u, chunks[index], sizeof(chunks[index])));
	}

	for(uint8_t index = 0; index < 8; index++) {
		TEST_ASSERT_EQUAL_UINT8_ARRAY(chunks[index],
			bltest::flashAt(config::appBase + index * 64u), sizeof(chunks[index]));
	}
	TEST_ASSERT_EQUAL_UINT32(512, session.highWater());
}

void test_duplicate_frames_are_free_rather_than_fatal()
{
	session.beginErase(config::appBase);
	finishErase(session);

	uint8_t payload[64];
	fillPattern(payload, sizeof(payload), 0x90);

	TEST_ASSERT_EQUAL(Error::None, session.write(0, payload, sizeof(payload)));
	const uint32_t afterFirst = bltest::programmedWords();

	// The legacy profile sends every frame twice. Real flash raises PROGERR on the second write of
	// a double-word even when the value is identical, and the fake here does too -- so a session
	// that did not track written granules would fail on the second frame of every upload.
	for(uint8_t repeat = 0; repeat < 4; repeat++) {
		TEST_ASSERT_EQUAL(Error::None, session.write(0, payload, sizeof(payload)));
	}

	TEST_ASSERT_EQUAL_UINT32(afterFirst, bltest::programmedWords());
	TEST_ASSERT_EQUAL_UINT8_ARRAY(payload, bltest::flashAt(config::appBase), sizeof(payload));
}

void test_overlapping_frames_of_different_sizes_agree()
{
	session.beginErase(config::appBase);
	finishErase(session);

	uint8_t big[128];
	fillPattern(big, sizeof(big), 0x10);

	// A v6 host streams 128-byte chunks; a repair pass, or an older host, may resend the same
	// bytes as 32-byte frames. Both must be accepted, and the overlap must not be reprogrammed.
	TEST_ASSERT_EQUAL(Error::None, session.write(0, big, sizeof(big)));
	const uint32_t afterBig = bltest::programmedWords();

	for(uint32_t offset = 0; offset < sizeof(big); offset += 32) {
		TEST_ASSERT_EQUAL(Error::None, session.write(offset, big + offset, 32));
	}

	TEST_ASSERT_EQUAL_UINT32(afterBig, bltest::programmedWords());
	TEST_ASSERT_EQUAL_UINT8_ARRAY(big, bltest::flashAt(config::appBase), sizeof(big));
}

void test_a_resumed_upload_accepts_what_is_already_in_flash()
{
	session.beginErase(config::appBase);
	finishErase(session);

	uint8_t payload[64];
	fillPattern(payload, sizeof(payload), 0x55);
	TEST_ASSERT_EQUAL(Error::None, session.write(0, payload, sizeof(payload)));

	// A reset mid-upload loses the bitmap but not the flash. The host resends from the start, and
	// every granule that already holds exactly the right bytes has to be accepted rather than
	// reported as a programming failure.
	Session resumed;
	TEST_ASSERT_EQUAL(Error::None, resumed.write(0, payload, sizeof(payload)));
	TEST_ASSERT_EQUAL_UINT32(64, resumed.highWater());
}

void test_writing_different_bytes_over_programmed_flash_is_refused()
{
	session.beginErase(config::appBase);
	finishErase(session);

	uint8_t first[64];
	uint8_t second[64];
	fillPattern(first, sizeof(first), 0x01);
	fillPattern(second, sizeof(second), 0x80);

	TEST_ASSERT_EQUAL(Error::None, session.write(0, first, sizeof(first)));

	// Same offset, different content, no erase in between. Real flash cannot do it, so saying so
	// is better than programming a bitwise-AND of the two and calling it success.
	Session fresh;
	TEST_ASSERT_EQUAL(Error::Program, fresh.write(0, second, sizeof(second)));
}

// ---- Bounds ------------------------------------------------------------------------------------

void test_offsets_must_be_double_word_aligned()
{
	session.beginErase(config::appBase);
	finishErase(session);

	uint8_t payload[8] = {1, 2, 3, 4, 5, 6, 7, 8};
	for(uint32_t offset = 1; offset < 8; offset++) {
		TEST_ASSERT_EQUAL(Error::Align, session.write(offset, payload, sizeof(payload)));
	}
	TEST_ASSERT_EQUAL(Error::None, session.write(8, payload, sizeof(payload)));
}

void test_nothing_may_be_written_past_the_application_bank()
{
	session.beginErase(config::appBase);
	finishErase(session);

	uint8_t payload[64];
	fillPattern(payload, sizeof(payload), 0x22);

	const uint32_t cap = session.capacity();
	TEST_ASSERT_EQUAL_UINT32(config::appCap, cap);

	// The last legal write ends exactly at the first durable page.
	TEST_ASSERT_EQUAL(Error::None, session.write(cap - 64, payload, 64));
	// One granule further is not.
	TEST_ASSERT_EQUAL(Error::Bounds, session.write(cap, payload, 8));
	TEST_ASSERT_EQUAL(Error::Bounds, session.write(cap - 8, payload, 64));

	// And an enormous length cannot wrap past the check. Written as
	// `length > cap - offset` rather than `offset + length > cap` for exactly this.
	TEST_ASSERT_EQUAL(Error::Bounds, session.write(0, payload, 0xFFFFFFF8u));
	TEST_ASSERT_EQUAL(Error::Bounds, session.write(0xFFFFFFF8u, payload, 8));

	// The durable pages are untouched by every one of those attempts.
	TEST_ASSERT_TRUE(regionBlank(PORTAL_PERSIST_IDENTITY, PORTAL_FLASH_PAGE_BYTES));
}

void test_the_legacy_base_has_a_smaller_bank()
{
	session.beginErase(config::appBaseLegacy);
	finishErase(session);

	uint8_t payload[8] = {9, 9, 9, 9, 9, 9, 9, 9};
	TEST_ASSERT_EQUAL_UINT32(config::appCapLegacy, session.capacity());
	TEST_ASSERT_EQUAL(Error::None, session.write(config::appCapLegacy - 8, payload, 8));
	TEST_ASSERT_EQUAL(Error::Bounds, session.write(config::appCapLegacy, payload, 8));
}

void test_writes_are_refused_while_erasing()
{
	session.beginErase(config::appBase);
	session.eraseStep();
	TEST_ASSERT_TRUE(session.isErasing());

	uint8_t payload[8] = {1, 2, 3, 4, 5, 6, 7, 8};
	TEST_ASSERT_EQUAL(Error::Busy, session.write(0, payload, sizeof(payload)));
}

void test_a_short_final_payload_is_padded_with_the_erased_value()
{
	session.beginErase(config::appBase);
	finishErase(session);

	// The v4/v5 bootloader advanced a raw uint64_t* across the caller's buffer regardless of
	// alignment, so a final chunk whose length was not a multiple of 8 read past a stack VLA and
	// programmed whatever was there. The image tail was then non-deterministic, and a byte-exact
	// readback check failed on exactly those bytes even after a perfect transfer.
	const uint8_t payload[4] = {0xAA, 0xBB, 0xCC, 0xDD};
	TEST_ASSERT_EQUAL(Error::None, session.write(0, payload, sizeof(payload)));

	const uint8_t * written = bltest::flashAt(config::appBase);
	TEST_ASSERT_EQUAL_UINT8_ARRAY(payload, written, sizeof(payload));
	for(uint32_t index = sizeof(payload); index < 8; index++) {
		TEST_ASSERT_EQUAL_UINT8(0xFF, written[index]);
	}
}

// ---- Declarations -------------------------------------------------------------------------------

void test_begin_validates_its_parameters()
{
	// Length must be non-zero, fit the bank, and be a whole number of double-words.
	TEST_ASSERT_EQUAL(Error::BadParam, session.declare(0, 1, 128, config::appBase));
	TEST_ASSERT_EQUAL(Error::BadParam, session.declare(config::appCap + 8, 1, 128, config::appBase));
	TEST_ASSERT_EQUAL(Error::BadParam, session.declare(129, 1, 128, config::appBase));

	// Chunk size likewise, and it may not exceed what the decode buffer can hold.
	TEST_ASSERT_EQUAL(Error::BadParam, session.declare(1024, 1, 0, config::appBase));
	TEST_ASSERT_EQUAL(Error::BadParam, session.declare(1024, 1, 100, config::appBase));
	TEST_ASSERT_EQUAL(Error::BadParam,
		session.declare(1024, 1, config::chunkMax + 8, config::appBase));

	// The base has to be one of the two an application is ever linked for.
	TEST_ASSERT_EQUAL(Error::BadParam, session.declare(1024, 1, 128, config::appBase + 0x1000));

	TEST_ASSERT_EQUAL(Error::None, session.declare(1024, 0xDEADBEEF, 128, config::appBase));
	TEST_ASSERT_TRUE(session.declared());
	TEST_ASSERT_EQUAL_UINT32(1024, session.length());
	TEST_ASSERT_EQUAL_UINT32(0xDEADBEEF, session.crc32());
	TEST_ASSERT_EQUAL_UINT32(128, session.chunkBytes());

	// A legacy-base declaration is bounded by the legacy bank, which is 8 kB smaller.
	Session legacy;
	TEST_ASSERT_EQUAL(Error::BadParam,
		legacy.declare(config::appCapLegacy + 8, 1, 128, config::appBaseLegacy));
	TEST_ASSERT_EQUAL(Error::None,
		legacy.declare(config::appCapLegacy, 1, 128, config::appBaseLegacy));
}

// ---- The received map ---------------------------------------------------------------------------

void test_the_map_reports_whole_chunks_only()
{
	session.beginErase(config::appBase);
	finishErase(session);
	TEST_ASSERT_EQUAL(Error::None, session.declare(512, 0, 128, config::appBase));

	uint8_t payload[128];
	fillPattern(payload, sizeof(payload), 0x30);

	// Chunks 0 and 2 complete, chunk 1 only half arrived, chunk 3 missing.
	TEST_ASSERT_EQUAL(Error::None, session.write(0, payload, 128));
	TEST_ASSERT_EQUAL(Error::None, session.write(128, payload, 64));
	TEST_ASSERT_EQUAL(Error::None, session.write(256, payload, 128));

	uint8_t bits[config::bitmapBytes];
	const size_t bytes = session.bitmap().renderChunks(128, 512, bits, sizeof(bits));
	TEST_ASSERT_EQUAL_size_t(1, bytes);
	// A half-received chunk reads as missing, so the host resends the whole thing rather than
	// leaving a hole it has been told is filled.
	TEST_ASSERT_EQUAL_UINT8(0b0000'0101, bits[0]);
}

void test_the_map_can_be_rendered_at_a_different_granularity()
{
	session.beginErase(config::appBase);
	finishErase(session);

	uint8_t payload[32];
	fillPattern(payload, sizeof(payload), 0x70);
	// Bytes 0..95 written, 96..127 not: a boundary that falls differently at each granularity.
	TEST_ASSERT_EQUAL(Error::None, session.write(0, payload, 32));
	TEST_ASSERT_EQUAL(Error::None, session.write(32, payload, 32));
	TEST_ASSERT_EQUAL(Error::None, session.write(64, payload, 32));

	// Tracking granules rather than chunks is what makes this possible: the host may ask at
	// whatever size it is currently sending, including one it did not start with. The same
	// underlying state answers all three questions, and each answer is only "complete" where the
	// whole chunk really did arrive.
	uint8_t bits[config::bitmapBytes];

	// Four 32-byte chunks: the first three arrived.
	TEST_ASSERT_EQUAL_size_t(1, session.bitmap().renderChunks(32, 128, bits, sizeof(bits)));
	TEST_ASSERT_EQUAL_UINT8(0b0000'0111, bits[0]);

	// Two 64-byte chunks: the first is covered by the first two writes; the second is half there.
	TEST_ASSERT_EQUAL_size_t(1, session.bitmap().renderChunks(64, 128, bits, sizeof(bits)));
	TEST_ASSERT_EQUAL_UINT8(0b0000'0001, bits[0]);

	// One 128-byte chunk, three quarters received, which is not received.
	TEST_ASSERT_EQUAL_size_t(1, session.bitmap().renderChunks(128, 128, bits, sizeof(bits)));
	TEST_ASSERT_EQUAL_UINT8(0b0000'0000, bits[0]);
}

void test_a_full_bank_map_fits_one_frame()
{
	session.beginErase(config::appBase);
	finishErase(session);

	uint8_t bits[config::bitmapBytes];
	const size_t bytes = session.bitmap().renderChunks(config::chunkMax, config::appCap,
		bits, sizeof(bits));
	// 424 chunks of 256 bytes -> 53 bytes, which is comfortably inside a `bin8` header's 255 and
	// inside the repeater's 2048-byte frame limit.
	TEST_ASSERT_EQUAL_size_t(53, bytes);
	for(size_t index = 0; index < bytes; index++) {
		TEST_ASSERT_EQUAL_UINT8(0x00, bits[index]);
	}
}

// ---- Runner -------------------------------------------------------------------------------------

int main()
{
	UNITY_BEGIN();

	RUN_TEST(test_erase_covers_the_whole_bank_one_page_at_a_time);
	RUN_TEST(test_erase_never_reaches_the_durable_pages);
	RUN_TEST(test_erase_never_reaches_the_bootloader_itself);
	RUN_TEST(test_a_legacy_erase_still_clears_the_new_bank);
	RUN_TEST(test_a_page_that_will_not_erase_stops_rather_than_looping);

	RUN_TEST(test_a_simple_sequential_upload_lands_byte_for_byte);
	RUN_TEST(test_frames_may_arrive_in_any_order);
	RUN_TEST(test_duplicate_frames_are_free_rather_than_fatal);
	RUN_TEST(test_overlapping_frames_of_different_sizes_agree);
	RUN_TEST(test_a_resumed_upload_accepts_what_is_already_in_flash);
	RUN_TEST(test_writing_different_bytes_over_programmed_flash_is_refused);

	RUN_TEST(test_offsets_must_be_double_word_aligned);
	RUN_TEST(test_nothing_may_be_written_past_the_application_bank);
	RUN_TEST(test_the_legacy_base_has_a_smaller_bank);
	RUN_TEST(test_writes_are_refused_while_erasing);
	RUN_TEST(test_a_short_final_payload_is_padded_with_the_erased_value);

	RUN_TEST(test_begin_validates_its_parameters);

	RUN_TEST(test_the_map_reports_whole_chunks_only);
	RUN_TEST(test_the_map_can_be_rendered_at_a_different_granularity);
	RUN_TEST(test_a_full_bank_map_fits_one_frame);

	return UNITY_END();
}
