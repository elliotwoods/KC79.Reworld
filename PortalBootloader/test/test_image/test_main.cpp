// Which application to start, and when to refuse to start any.
//
// This is the decision that can brick a board. Starting an image linked for the wrong bank does
// not fail at the jump -- it fails later, at the first absolute address the code touches, as a
// hard fault with no relationship to the mistake. A board that does that on every boot is
// indistinguishable from dead hardware, and the only recovery is a debug probe.
//
// So the rule is that the bootloader refuses anything it cannot positively identify, and the tests
// below are mostly about the refusals.

#include <unity.h>

#include "bl/image.hpp"
#include "fake_hw.hpp"

#include <initializer_list>
#include <string.h>

using namespace bl;

namespace {
	const char * const version = "Portal v2026-08-25_19.19 ea08436+";

	/// A well-formed application at `base`, declaring `declaredBase` in its descriptor.
	void installApplication(uint32_t base, uint32_t declaredBase, bool descriptor)
	{
		bltest::preloadApplication(base, 0x241, descriptor, declaredBase, version);
	}
}

void setUp()
{
	bltest::reset();
}

void tearDown() {}

// ---- Vector table sanity --------------------------------------------------------------------

void test_a_blank_bank_has_no_vector_table()
{
	TEST_ASSERT_FALSE(vectorTableValid(config::appBase));
	TEST_ASSERT_FALSE(vectorTableValid(config::appBaseLegacy));
}

void test_a_plausible_vector_table_is_accepted()
{
	installApplication(config::appBase, config::appBase, true);
	TEST_ASSERT_TRUE(vectorTableValid(config::appBase));
}

void test_a_stack_pointer_outside_sram_is_refused()
{
	installApplication(config::appBase, config::appBase, true);
	for(uint32_t bad : {0x0800'0000u, 0x2001'0000u, (uint32_t) PORTAL_RAM_BASE}) {
		bltest::preload(config::appBase, (const uint8_t *) &bad, 4);
		TEST_ASSERT_FALSE(vectorTableValid(config::appBase));
	}

	// The top of SRAM *is* legal: the stack grows down, so `_estack` is one past the last byte and
	// that is exactly what a linker emits.
	const uint32_t top = PORTAL_RAM_END;
	bltest::preload(config::appBase, (const uint8_t *) &top, 4);
	TEST_ASSERT_TRUE(vectorTableValid(config::appBase));
}

void test_a_misaligned_stack_pointer_is_refused()
{
	installApplication(config::appBase, config::appBase, true);
	const uint32_t odd = PORTAL_RAM_END - 1;
	bltest::preload(config::appBase, (const uint8_t *) &odd, 4);
	TEST_ASSERT_FALSE(vectorTableValid(config::appBase));
}

void test_a_reset_vector_without_the_thumb_bit_is_refused()
{
	installApplication(config::appBase, config::appBase, true);
	const uint32_t even = config::appBase + 0x240;
	bltest::preload(config::appBase + 4, (const uint8_t *) &even, 4);
	// Not a Cortex-M entry point. Branching to it faults immediately.
	TEST_ASSERT_FALSE(vectorTableValid(config::appBase));
}

void test_a_reset_vector_outside_the_bank_is_refused()
{
	installApplication(config::appBase, config::appBase, true);
	for(uint32_t bad : {PORTAL_FLASH_BASE + 1u, PORTAL_PERSIST_IDENTITY + 1u, 0x2000'0001u}) {
		bltest::preload(config::appBase + 4, (const uint8_t *) &bad, 4);
		TEST_ASSERT_FALSE(vectorTableValid(config::appBase));
	}
}

// ---- The descriptor -------------------------------------------------------------------------

void test_the_descriptor_is_read_from_a_fixed_offset()
{
	installApplication(config::appBase, config::appBase, true);
	const portal_app_descriptor_t * descriptor = descriptorAt(config::appBase);
	TEST_ASSERT_NOT_NULL(descriptor);
	TEST_ASSERT_EQUAL_UINT32(config::appBase, descriptor->app_base);
	TEST_ASSERT_EQUAL_STRING(version, descriptor->version);

	// The G070's vector table is 46 entries = 0xB8 bytes, so 0xC0 clears it with room to spare.
	// The offset is fixed rather than "wherever the linker put it": the bootloader reads it out of
	// an image it did not build, and orphan-section placement is not a contract.
	TEST_ASSERT_EQUAL_UINT32(0xC0, PORTAL_APP_DESCRIPTOR_OFFSET);
	TEST_ASSERT_TRUE(PORTAL_APP_DESCRIPTOR_OFFSET >= 46 * 4);
}

void test_a_missing_or_damaged_descriptor_reads_as_absent()
{
	installApplication(config::appBase, config::appBase, false);
	TEST_ASSERT_NULL(descriptorAt(config::appBase));

	installApplication(config::appBase, config::appBase, true);
	const uint8_t wrong = 'X';
	bltest::preload(config::appBase + PORTAL_APP_DESCRIPTOR_OFFSET, &wrong, 1);
	TEST_ASSERT_NULL(descriptorAt(config::appBase));
}

// ---- The decision table -----------------------------------------------------------------------

void test_a_new_base_image_with_a_matching_descriptor_runs()
{
	installApplication(config::appBase, config::appBase, true);
	const RunDecision decision = decideRun(0, 0, config::appBase);
	TEST_ASSERT_TRUE(decision.ok);
	TEST_ASSERT_EQUAL_UINT32(config::appBase, decision.base);
	TEST_ASSERT_EQUAL(Error::None, decision.error);
}

void test_a_new_base_image_without_a_descriptor_is_refused()
{
	// This is the case the descriptor exists for. A legacy host that uploaded a 0x08006000-linked
	// image through the v6 path would land it here, where its vector table looks entirely
	// plausible and every absolute address in it is 8 kB wrong.
	installApplication(config::appBase, config::appBase, false);
	const RunDecision decision = decideRun(0, 0, config::appBase);
	TEST_ASSERT_FALSE(decision.ok);
	TEST_ASSERT_EQUAL(Error::DescriptorMissing, decision.error);
}

void test_an_image_declaring_the_wrong_base_is_refused()
{
	installApplication(config::appBase, config::appBaseLegacy, true);
	const RunDecision decision = decideRun(0, 0, config::appBase);
	TEST_ASSERT_FALSE(decision.ok);
	TEST_ASSERT_EQUAL(Error::DescriptorBase, decision.error);
}

void test_a_legacy_image_runs_when_the_new_bank_is_blank()
{
	// The transition state: a board whose bootloader has just been replaced in the field, and
	// whose application is still the one linked for the old base. Without this fallback the
	// update would be a flag day -- every board would go dark between replacing its bootloader
	// and re-uploading its application.
	installApplication(config::appBaseLegacy, 0, false);
	const RunDecision decision = decideRun(0, 0, config::appBase);
	TEST_ASSERT_TRUE(decision.ok);
	TEST_ASSERT_EQUAL_UINT32(config::appBaseLegacy, decision.base);
}

void test_a_legacy_image_with_a_matching_descriptor_also_runs()
{
	installApplication(config::appBaseLegacy, config::appBaseLegacy, true);
	const RunDecision decision = decideRun(0, 0, config::appBase);
	TEST_ASSERT_TRUE(decision.ok);
	TEST_ASSERT_EQUAL_UINT32(config::appBaseLegacy, decision.base);
}

void test_a_half_uploaded_new_image_does_not_expose_the_legacy_fallback()
{
	// A working legacy application, and an upload into the new bank that stopped after its first
	// frames. The new bank now has a plausible vector table and nothing behind it.
	installApplication(config::appBaseLegacy, 0, false);
	installApplication(config::appBase, config::appBase, false);

	// Falling through to the legacy image here would be worse than refusing: the board would come
	// up working, and the operator would have no signal that the update failed.
	const RunDecision decision = decideRun(0, 0, config::appBase);
	TEST_ASSERT_FALSE(decision.ok);
	TEST_ASSERT_EQUAL(Error::DescriptorMissing, decision.error);
}

void test_a_blank_device_refuses_to_run_anything()
{
	const RunDecision decision = decideRun(0, 0, config::appBase);
	TEST_ASSERT_FALSE(decision.ok);
	TEST_ASSERT_EQUAL(Error::NoApp, decision.error);
}

void test_a_declared_crc_that_does_not_match_refuses_before_anything_else()
{
	installApplication(config::appBase, config::appBase, true);

	// A session that declared a CRC is the strongest available statement that the image is whole.
	// An upload that lost its last frame leaves a perfectly plausible vector table behind, so the
	// CRC has to be checked before the image's shape is even considered.
	const uint32_t length = 0x400;
    const uint32_t actual = crcOverFlash(config::appBase, length);
	const RunDecision bad = decideRun(length, actual ^ 0xFFFFFFFFu, config::appBase);
	TEST_ASSERT_FALSE(bad.ok);
	TEST_ASSERT_EQUAL(Error::ImageCrc, bad.error);

	const RunDecision good = decideRun(length, actual, config::appBase);
	TEST_ASSERT_TRUE(good.ok);
}

void test_the_crc_matches_the_shared_implementation()
{
	// The same function the host uses to compute what it declares in `begin`, and the same one
	// PortalFW uses for its persistent records. A disagreement here would look like a corrupted
	// upload on every board.
	const uint8_t vector[] = {'1', '2', '3', '4', '5', '6', '7', '8', '9'};
	bltest::preload(config::appBase, vector, sizeof(vector));
	TEST_ASSERT_EQUAL_HEX32(0xE3069283, crcOverFlash(config::appBase, sizeof(vector)));
}

void test_the_crc_kicks_the_watchdog_as_it_goes()
{
	// A full bank is about 70 ms of CRC at 64 MHz against a ~4.1 s watchdog, so this is not close
	// -- but the routine is also reached from contexts that have already spent part of the period,
	// and a watchdog reset in the middle of `verify` would look like a failed upload.
	const uint32_t before = bltest::watchdogKicks();
	crcOverFlash(config::appBase, config::appCap);
	TEST_ASSERT_GREATER_THAN_UINT32(before + 50, bltest::watchdogKicks());
}

// ---- Runner ------------------------------------------------------------------------------------

int main()
{
	UNITY_BEGIN();

	RUN_TEST(test_a_blank_bank_has_no_vector_table);
	RUN_TEST(test_a_plausible_vector_table_is_accepted);
	RUN_TEST(test_a_stack_pointer_outside_sram_is_refused);
	RUN_TEST(test_a_misaligned_stack_pointer_is_refused);
	RUN_TEST(test_a_reset_vector_without_the_thumb_bit_is_refused);
	RUN_TEST(test_a_reset_vector_outside_the_bank_is_refused);

	RUN_TEST(test_the_descriptor_is_read_from_a_fixed_offset);
	RUN_TEST(test_a_missing_or_damaged_descriptor_reads_as_absent);

	RUN_TEST(test_a_new_base_image_with_a_matching_descriptor_runs);
	RUN_TEST(test_a_new_base_image_without_a_descriptor_is_refused);
	RUN_TEST(test_an_image_declaring_the_wrong_base_is_refused);
	RUN_TEST(test_a_legacy_image_runs_when_the_new_bank_is_blank);
	RUN_TEST(test_a_legacy_image_with_a_matching_descriptor_also_runs);
	RUN_TEST(test_a_half_uploaded_new_image_does_not_expose_the_legacy_fallback);
	RUN_TEST(test_a_blank_device_refuses_to_run_anything);
	RUN_TEST(test_a_declared_crc_that_does_not_match_refuses_before_anything_else);
	RUN_TEST(test_the_crc_matches_the_shared_implementation);
	RUN_TEST(test_the_crc_kicks_the_watchdog_as_it_goes);

	return UNITY_END();
}
