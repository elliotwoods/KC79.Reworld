// Reading a board's provisioning serial out of the identity page, and the layout that page shares
// with three other implementations.
//
// The bootloader only reads this. It never writes it, never erases it, and its erase is bounded so
// it cannot reach it by accident -- which is the entire point of the rewrite, and is asserted in
// test_session. What is checked here is that the *reading* agrees, byte for byte, with the two
// implementations that do the writing: `PortalFW/src/PersistentStorage.cpp` on the board and
// `PortalFlasher/crates/portal-swd/src/persistent.rs` on the host.
//
// The golden vectors below are lifted from that Rust test suite. Three implementations of one
// format is two too many, and the only thing keeping them honest is that all three are pinned to
// the same bytes.

#include <unity.h>

#include "bl/identity.hpp"
#include "bl/config.hpp"
#include "fake_hw.hpp"

#include "portal_crc32c.h"
#include "portal_flash_layout.h"

#include <initializer_list>
#include <string.h>

using namespace bl;

namespace {
	/// The MCU UID the fake reports, and the one the golden record below is bound to.
	const uint32_t goldenUid[3] = {0x1122'3344u, 0x5566'7788u, 0x99AA'BBCCu};

	/// `exact_golden_identity_layout` from `portal-swd`'s `persistent.rs`: generation 0x01020304,
	/// that UID, serial 123456. Written out here as the hex string that test asserts, so a change
	/// on either side is a failure on both.
	const char * goldenHex =
		"4b435052563030310100010004030201040000004433221188776655ccbbaa9940e2010"
		"0ffffffffffffffffffffffffffffffffffffffffffffffff2758ad80";

	void hexToBytes(const char * hex, uint8_t * out, size_t count)
	{
		for(size_t index = 0; index < count; index++) {
			auto nibble = [](char c) -> uint8_t {
				if(c >= '0' && c <= '9') return (uint8_t) (c - '0');
				if(c >= 'a' && c <= 'f') return (uint8_t) (c - 'a' + 10);
				return (uint8_t) (c - 'A' + 10);
			};
			out[index] = (uint8_t) ((nibble(hex[index * 2]) << 4) | nibble(hex[index * 2 + 1]));
		}
	}

	/// Build a record the way the application does, so the tests can vary one field at a time.
	void makeRecord(uint8_t out[64], uint32_t generation, const uint32_t uid[3], uint32_t serial,
		uint16_t kind = 1, uint32_t payloadLength = 4)
	{
		memset(out, 0xFF, 64);
		const uint64_t magic = 0x313030565250434BULL; // "KCPRV001", little-endian
		memcpy(out, &magic, 8);
		const uint16_t schema = 1;
		memcpy(out + 8, &schema, 2);
		memcpy(out + 10, &kind, 2);
		memcpy(out + 12, &generation, 4);
		memcpy(out + 16, &payloadLength, 4);
		memcpy(out + 20, uid, 12);
		memcpy(out + 32, &serial, 4);
		const uint32_t crc = portal_crc32c(out, 60);
		memcpy(out + 60, &crc, 4);
	}

	void install(const uint8_t record[64], uint32_t slot = 0)
	{
		bltest::preload(PORTAL_PERSIST_IDENTITY + slot * 64u, record, 64);
	}
}

void setUp()
{
	bltest::reset();
	bltest::setUid(goldenUid[0], goldenUid[1], goldenUid[2]);
}

void tearDown() {}

// ---- The shared format ---------------------------------------------------------------------------

void test_the_golden_record_from_the_host_test_suite_reads_back()
{
	uint8_t record[64];
	hexToBytes(goldenHex, record, sizeof(record));
	install(record);

	const Identity identity = readIdentity();
	TEST_ASSERT_TRUE_MESSAGE(identity.valid, "the host's golden record did not read back");
	TEST_ASSERT_EQUAL_UINT32(123456, identity.serial);
	TEST_ASSERT_EQUAL_UINT32(0x01020304, identity.generation);
	TEST_ASSERT_FALSE(identity.foreignUid);
}

void test_a_record_built_here_matches_the_golden_bytes()
{
	// The other direction: what this project believes the format is, compared with what the host
	// actually writes. A disagreement in either direction would look like a corrupted page.
	uint8_t built[64];
	makeRecord(built, 0x01020304, goldenUid, 123456);

	uint8_t golden[64];
	hexToBytes(goldenHex, golden, sizeof(golden));

	TEST_ASSERT_EQUAL_UINT8_ARRAY(golden, built, sizeof(golden));
}

void test_the_crc_is_castagnoli()
{
	// The one field most likely to be "corrected" to the more familiar zlib CRC-32, which would
	// silently invalidate every record ever written.
	TEST_ASSERT_EQUAL_HEX32(0xE3069283, portal_crc32c((const uint8_t *) "123456789", 9));
}

// ---- Reading ---------------------------------------------------------------------------------------

void test_a_blank_page_has_no_identity()
{
	const Identity identity = readIdentity();
	TEST_ASSERT_FALSE(identity.valid);
	TEST_ASSERT_FALSE(identity.foreignUid);
	TEST_ASSERT_EQUAL_UINT32(0, identity.serial);
}

void test_the_highest_generation_wins_wherever_it_sits()
{
	// The journal is append-only, and a compaction rewrites slot 0 -- so the newest record is the
	// one with the highest generation, not the one in the highest slot.
	uint8_t record[64];
	makeRecord(record, 4, goldenUid, 40);
	install(record, 0);
	makeRecord(record, 9, goldenUid, 90);
	install(record, 1);
	makeRecord(record, 6, goldenUid, 60);
	install(record, 2);

	const Identity identity = readIdentity();
	TEST_ASSERT_TRUE(identity.valid);
	TEST_ASSERT_EQUAL_UINT32(90, identity.serial);
	TEST_ASSERT_EQUAL_UINT32(9, identity.generation);
}

void test_a_torn_record_is_skipped_rather_than_believed()
{
	uint8_t good[64];
	makeRecord(good, 3, goldenUid, 55);
	install(good, 0);

	uint8_t torn[64];
	makeRecord(torn, 9, goldenUid, 999);
	torn[40] ^= 0x01; // a bit flip inside the CRC's coverage
	install(torn, 1);

	// The torn record has the higher generation. Trusting it would give the board somebody else's
	// serial number; the CRC is what stops that.
	const Identity identity = readIdentity();
	TEST_ASSERT_TRUE(identity.valid);
	TEST_ASSERT_EQUAL_UINT32(55, identity.serial);
}

void test_a_record_belonging_to_another_mcu_is_not_ours_to_answer_for()
{
	const uint32_t otherUid[3] = {1, 2, 3};
	uint8_t record[64];
	makeRecord(record, 5, otherUid, 4242);
	install(record);

	// A page cloned between boards. Reporting the serial would give two boards the same identity,
	// which on a bus addressed by serial is worse than having none.
	const Identity identity = readIdentity();
	TEST_ASSERT_FALSE(identity.valid);
	TEST_ASSERT_TRUE(identity.foreignUid);
	TEST_ASSERT_EQUAL_UINT32(0, identity.serial);
}

void test_the_reserved_serials_are_refused()
{
	for(uint32_t serial : {0u, 0xFFFFFFFFu}) {
		bltest::reset();
		bltest::setUid(goldenUid[0], goldenUid[1], goldenUid[2]);
		uint8_t record[64];
		makeRecord(record, 5, goldenUid, serial);
		install(record);
		// 0 is what an unprovisioned board reports and 0xFFFFFFFF is erased flash. Neither was
		// ever issued, and a board answering a selector for either would be answering for a
		// whole rack.
		TEST_ASSERT_FALSE(readIdentity().valid);
	}
}

void test_a_settings_record_is_not_read_as_an_identity()
{
	// Kind 2 lives in the settings pages, but a mis-addressed write could land one here.
	uint8_t record[64];
	makeRecord(record, 5, goldenUid, 4242, 2, 17);
	install(record);
	TEST_ASSERT_FALSE(readIdentity().valid);
}

void test_the_whole_page_is_scanned()
{
	// 32 slots of 64 bytes. A record in the last one is as valid as one in the first.
	uint8_t record[64];
	makeRecord(record, 1, goldenUid, 31337);
	install(record, (PORTAL_FLASH_PAGE_BYTES / 64) - 1);

	const Identity identity = readIdentity();
	TEST_ASSERT_TRUE(identity.valid);
	TEST_ASSERT_EQUAL_UINT32(31337, identity.serial);
}

// ---- Runner -----------------------------------------------------------------------------------------

int main()
{
	UNITY_BEGIN();

	RUN_TEST(test_the_golden_record_from_the_host_test_suite_reads_back);
	RUN_TEST(test_a_record_built_here_matches_the_golden_bytes);
	RUN_TEST(test_the_crc_is_castagnoli);

	RUN_TEST(test_a_blank_page_has_no_identity);
	RUN_TEST(test_the_highest_generation_wins_wherever_it_sits);
	RUN_TEST(test_a_torn_record_is_skipped_rather_than_believed);
	RUN_TEST(test_a_record_belonging_to_another_mcu_is_not_ours_to_answer_for);
	RUN_TEST(test_the_reserved_serials_are_refused);
	RUN_TEST(test_a_settings_record_is_not_read_as_an_identity);
	RUN_TEST(test_the_whole_page_is_scanned);

	return UNITY_END();
}
