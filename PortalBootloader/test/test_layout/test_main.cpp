// The two structures that cross an image boundary, and the map they live in.
//
// `portal_flash_layout.h` already `static_assert`s the *size* of both structs in every build that
// includes it. Size is not enough: two fields swapped, or a `uint32_t` traded for two `uint16_t`s,
// leaves the size identical and changes what the other image reads. So the offsets are pinned here
// individually, and the handoff block is pinned as bytes -- because it is written by the
// application, read by the bootloader, and the two are separately linked images built by different
// compilers with different flags.

#include <unity.h>

#include "bl/config.hpp"
#include "fake_hw.hpp"

#include "portal_crc32c.h"
#include "portal_flash_layout.h"

#include <stddef.h>
#include <string.h>

void setUp() { bltest::reset(); }
void tearDown() {}

// ---- The handoff block -----------------------------------------------------------------------

void test_the_handoff_block_has_the_layout_both_images_assume()
{
	TEST_ASSERT_EQUAL_size_t(32, sizeof(portal_handoff_t));
	TEST_ASSERT_EQUAL_size_t(0x00, offsetof(portal_handoff_t, magic));
	TEST_ASSERT_EQUAL_size_t(0x04, offsetof(portal_handoff_t, version));
	TEST_ASSERT_EQUAL_size_t(0x05, offsetof(portal_handoff_t, request));
	TEST_ASSERT_EQUAL_size_t(0x06, offsetof(portal_handoff_t, id));
	TEST_ASSERT_EQUAL_size_t(0x07, offsetof(portal_handoff_t, flags));
	TEST_ASSERT_EQUAL_size_t(0x08, offsetof(portal_handoff_t, serial));
	TEST_ASSERT_EQUAL_size_t(0x0C, offsetof(portal_handoff_t, arg0));
	TEST_ASSERT_EQUAL_size_t(0x1C, offsetof(portal_handoff_t, crc32c));
}

void test_a_golden_handoff_block_is_byte_exact()
{
	// Board 7, serial 73001, asking to stay in the bootloader. Written out as bytes rather than
	// round-tripped, so a change to the layout fails here rather than passing against itself.
	portal_handoff_t block;
	memset(&block, 0, sizeof(block));
	block.magic = PORTAL_HANDOFF_MAGIC;
	block.version = PORTAL_HANDOFF_VERSION;
	block.request = PORTAL_HANDOFF_REQUEST_STAY;
	block.id = 7;
	block.flags = PORTAL_HANDOFF_FLAG_SERIAL_VALID;
	block.serial = 73001;
	block.crc32c = portal_crc32c((const uint8_t *) &block, offsetof(portal_handoff_t, crc32c));

	const uint8_t expected[32] = {
		0x4B, 0x37, 0x39, 0x48,             // magic, "K79H" little-endian
		0x01,                               // version
		0x01,                               // request: stay
		0x07,                               // id
		0x01,                               // flags: serial valid
		0x29, 0x1D, 0x01, 0x00,             // serial 73001 = 0x11D29
		0x00, 0x00, 0x00, 0x00,             // arg0
		0x00, 0x00, 0x00, 0x00,             // reserved[0]
		0x00, 0x00, 0x00, 0x00,             // reserved[1]
		0x00, 0x00, 0x00, 0x00,             // reserved[2]
		0x95, 0x4F, 0x12, 0x7D,             // crc32c over bytes 0..27 = 0x7D124F95
	};
	TEST_ASSERT_EQUAL_UINT8_ARRAY(expected, (const uint8_t *) &block, sizeof(expected));
}

void test_the_handoff_crc_rejects_a_single_flipped_bit()
{
	portal_handoff_t block;
	memset(&block, 0, sizeof(block));
	block.magic = PORTAL_HANDOFF_MAGIC;
	block.version = PORTAL_HANDOFF_VERSION;
	block.id = 7;
	block.crc32c = portal_crc32c((const uint8_t *) &block, offsetof(portal_handoff_t, crc32c));

	// This RAM holds whatever the last program left in it. Without the CRC, a stale pattern
	// resembling the magic would hand a board somebody else's bus address.
	uint8_t * bytes = (uint8_t *) &block;
	for(size_t index = 0; index < offsetof(portal_handoff_t, crc32c); index++) {
		bytes[index] ^= 0x01;
		const uint32_t recomputed =
			portal_crc32c(bytes, offsetof(portal_handoff_t, crc32c));
		TEST_ASSERT_NOT_EQUAL_MESSAGE(block.crc32c, recomputed, "a flipped bit went undetected");
		bytes[index] ^= 0x01;
	}
}

// ---- The application descriptor ------------------------------------------------------------------

void test_the_descriptor_has_the_layout_the_bootloader_reads()
{
	TEST_ASSERT_EQUAL_size_t(56, sizeof(portal_app_descriptor_t));
	TEST_ASSERT_EQUAL_size_t(0x00, offsetof(portal_app_descriptor_t, magic));
	TEST_ASSERT_EQUAL_size_t(0x08, offsetof(portal_app_descriptor_t, app_base));
	TEST_ASSERT_EQUAL_size_t(0x0C, offsetof(portal_app_descriptor_t, flags));
	TEST_ASSERT_EQUAL_size_t(0x10, offsetof(portal_app_descriptor_t, version));
	TEST_ASSERT_EQUAL_size_t(40, sizeof(((portal_app_descriptor_t *) 0)->version));

	// The magic carries no terminator: eight bytes, all of them significant.
	TEST_ASSERT_EQUAL_size_t(9, sizeof(PORTAL_APP_DESCRIPTOR_MAGIC));
	TEST_ASSERT_EQUAL_size_t(8, sizeof(((portal_app_descriptor_t *) 0)->magic));
}

// ---- The map --------------------------------------------------------------------------------------

void test_the_map_is_internally_consistent()
{
	// Every boundary is an erase boundary, so every one lands on a page.
	const uint32_t boundaries[] = {
		PORTAL_FLASH_BASE, PORTAL_APP_BASE, PORTAL_APP_BASE_LEGACY, PORTAL_APP_END,
		PORTAL_PERSIST_IDENTITY, PORTAL_PERSIST_SETTINGS_A, PORTAL_PERSIST_SETTINGS_B,
		PORTAL_FLASH_END,
	};
	for(uint32_t boundary : boundaries) {
		TEST_ASSERT_EQUAL_UINT32(0, boundary % PORTAL_FLASH_PAGE_BYTES);
	}

	// Each bootloader bank ends exactly where its application begins.
	TEST_ASSERT_EQUAL_UINT32(PORTAL_APP_BASE, PORTAL_FLASH_BASE + PORTAL_BOOTLOADER_BYTES);
	TEST_ASSERT_EQUAL_UINT32(PORTAL_APP_BASE_LEGACY,
		PORTAL_FLASH_BASE + PORTAL_BOOTLOADER_BYTES_LEGACY);

	// The three durable pages sit above the application and fill flash to the end.
	TEST_ASSERT_EQUAL_UINT32(PORTAL_APP_END, PORTAL_PERSIST_IDENTITY);
	TEST_ASSERT_EQUAL_UINT32(PORTAL_PERSIST_SETTINGS_A,
		PORTAL_PERSIST_IDENTITY + PORTAL_FLASH_PAGE_BYTES);
	TEST_ASSERT_EQUAL_UINT32(PORTAL_PERSIST_SETTINGS_B,
		PORTAL_PERSIST_SETTINGS_A + PORTAL_FLASH_PAGE_BYTES);
	TEST_ASSERT_EQUAL_UINT32(PORTAL_FLASH_END,
		PORTAL_PERSIST_SETTINGS_B + PORTAL_FLASH_PAGE_BYTES);

	// The handoff block occupies the very top of SRAM, which is what makes excluding it from a
	// linker script a matter of shortening RAM rather than carving a hole in it.
	TEST_ASSERT_EQUAL_UINT32(PORTAL_RAM_END, PORTAL_HANDOFF_ADDR + PORTAL_HANDOFF_BYTES);

	// The descriptor clears the G070's 46-entry vector table.
	TEST_ASSERT_TRUE(PORTAL_APP_DESCRIPTOR_OFFSET >= 46 * 4);

	// And the move is worth making: 8 kB more application.
	TEST_ASSERT_EQUAL_UINT32(108544, bl::config::appCap);
	TEST_ASSERT_EQUAL_UINT32(100352, bl::config::appCapLegacy);
	TEST_ASSERT_EQUAL_UINT32(PORTAL_BOOTLOADER_BYTES_LEGACY - PORTAL_BOOTLOADER_BYTES,
		bl::config::appCap - bl::config::appCapLegacy);
}

// ---- Runner ------------------------------------------------------------------------------------------

int main()
{
	UNITY_BEGIN();

	RUN_TEST(test_the_handoff_block_has_the_layout_both_images_assume);
	RUN_TEST(test_a_golden_handoff_block_is_byte_exact);
	RUN_TEST(test_the_handoff_crc_rejects_a_single_flipped_bit);

	RUN_TEST(test_the_descriptor_has_the_layout_the_bootloader_reads);

	RUN_TEST(test_the_map_is_internally_consistent);

	return UNITY_END();
}
