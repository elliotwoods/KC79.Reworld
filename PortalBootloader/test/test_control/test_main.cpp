// The bootloader as a whole: a host on one side, flash on the other, and every verb driven
// through the real state machine.
//
// The tests below are the ones that would otherwise be run with a board, a probe and a bus. What
// they cannot cover is timing against real hardware; what they do cover is every decision the
// firmware makes about what to erase, what to answer, and what to start.

#include <unity.h>

#include "bl/bootloader.hpp"
#include "bl/hw.hpp"
#include "fake_hw.hpp"
#include "frames.hpp"

#include "portal_crc32c.h"

#include <initializer_list>
#include <string.h>

using namespace bl;

namespace {
	constexpr int8_t ourId = 3;
	constexpr uint32_t ourSerial = 73001;

	bltest::DuplexStream stream;
	uint32_t now = 0;

	/// Run the bootloader until it is quiet: no frame pending, no erase in progress.
	void settle(Bootloader & bootloader, uint32_t maxTicks = 500)
	{
		for(uint32_t tick = 0; tick < maxTicks; tick++) {
			bootloader.tick(now);
			if(bootloader.phase() != Phase::Erasing && stream.queuedFrames() == 0) {
				// One more pass so a reply raised by the last frame is written.
				bootloader.tick(now);
				return;
			}
		}
	}

	void send(bltest::FrameBuilder & builder)
	{
		stream.deliver(builder.bytes(), builder.size());
	}

	bltest::Reply lastReply()
	{
		const bltest::Reply reply = bltest::readReply(stream);
		stream.clearSent();
		return reply;
	}

	/// A provisioning identity record, so the board has a serial to be selected by.
	void installIdentity(uint32_t serial)
	{
		uint8_t record[64];
		memset(record, 0xFF, sizeof(record));
		const uint64_t magic = 0x313030565250434BULL; // "KCPRV001"
		memcpy(record, &magic, 8);
		const uint16_t schema = 1;
		const uint16_t kind = 1;
		const uint32_t generation = 7;
		const uint32_t payloadLength = 4;
		memcpy(record + 8, &schema, 2);
		memcpy(record + 10, &kind, 2);
		memcpy(record + 12, &generation, 4);
		memcpy(record + 16, &payloadLength, 4);
		uint32_t uid[3];
		hw::uid(uid);
		memcpy(record + 20, uid, 12);
		memcpy(record + 32, &serial, 4);
		const uint32_t crc = portal_crc32c(record, 60);
		memcpy(record + 60, &crc, 4);
		bltest::preload(PORTAL_PERSIST_IDENTITY, record, sizeof(record));
	}

	/// A handoff block as the application writes it before resetting.
	void installHandoff(int8_t id, uint32_t serial, uint8_t request)
	{
		portal_handoff_t * block = hw::handoff();
		memset(block, 0, sizeof(*block));
		block->magic = PORTAL_HANDOFF_MAGIC;
		block->version = PORTAL_HANDOFF_VERSION;
		block->request = request;
		block->id = id;
		block->flags = PORTAL_HANDOFF_FLAG_SERIAL_VALID;
		block->serial = serial;
		block->crc32c = portal_crc32c((const uint8_t *) block,
			(uint32_t) offsetof(portal_handoff_t, crc32c));
	}
}

void setUp()
{
	bltest::reset();
	stream = bltest::DuplexStream();
	now = 1000;
}

void tearDown() {}

// ---- Identity and addressing --------------------------------------------------------------------

void test_the_address_comes_from_the_application_when_it_left_one()
{
	installHandoff(9, ourSerial, PORTAL_HANDOFF_REQUEST_STAY);
	Bootloader bootloader(stream);
	bootloader.begin(now);

	TEST_ASSERT_EQUAL_INT8(9, bootloader.address());
	TEST_ASSERT_EQUAL(IdSource::Handoff, bootloader.idSource());
	// An update is expected, so the board waits thirty seconds rather than three.
	TEST_ASSERT_EQUAL_UINT32(now + 30000, bootloader.deadline());
}

void test_the_request_is_consumed_but_the_identity_is_kept()
{
	installHandoff(9, ourSerial, PORTAL_HANDOFF_REQUEST_STAY);
	Bootloader bootloader(stream);
	bootloader.begin(now);

	// A watchdog reset mid-update must not lose the address the application gave us -- there is no
	// application left to ask again. The id survives; the thirty-second stay does not.
	const portal_handoff_t * block = hw::handoff();
	TEST_ASSERT_EQUAL_UINT8(PORTAL_HANDOFF_REQUEST_NONE, block->request);
	TEST_ASSERT_EQUAL_INT8(9, block->id);
	TEST_ASSERT_EQUAL_UINT32(portal_crc32c((const uint8_t *) block,
		(uint32_t) offsetof(portal_handoff_t, crc32c)), block->crc32c);

	Bootloader second(stream);
	second.begin(now);
	TEST_ASSERT_EQUAL_INT8(9, second.address());
	TEST_ASSERT_EQUAL_UINT32(now + 3000, second.deadline());
}

void test_a_corrupt_handoff_falls_back_to_the_switches()
{
	installHandoff(9, ourSerial, PORTAL_HANDOFF_REQUEST_STAY);
	hw::handoff()->crc32c ^= 1;
	bltest::setDip(4);

	Bootloader bootloader(stream);
	bootloader.begin(now);
	// DIP + 1, exactly as the application maps the same four pins, so a board answers on the same
	// address whichever image is running.
	TEST_ASSERT_EQUAL_INT8(5, bootloader.address());
	TEST_ASSERT_EQUAL(IdSource::Dip, bootloader.idSource());
}

// ---- status ---------------------------------------------------------------------------------------

void test_status_reports_the_board()
{
	installIdentity(ourSerial);
	installHandoff(ourId, ourSerial, PORTAL_HANDOFF_REQUEST_STAY);
	bltest::preloadApplication(config::appBase, 0x241, true, config::appBase, "Portal v2026-08-25");

	Bootloader bootloader(stream);
	bootloader.begin(now);

	bltest::FrameBuilder builder(ourId, 0, 11);
	bltest::controlBegin(builder, "status", 0);
	builder.finish();
	send(builder);
	settle(bootloader);

	const bltest::Reply reply = lastReply();
	TEST_ASSERT_TRUE(reply.present);
	TEST_ASSERT_EQUAL_STRING("status", reply.verb);
	TEST_ASSERT_TRUE(reply.trailerOk);
	TEST_ASSERT_EQUAL_UINT8(11, reply.seq);
	TEST_ASSERT_EQUAL_INT32(0, reply.target);
	TEST_ASSERT_EQUAL_INT32(ourId, reply.source);

	TEST_ASSERT_EQUAL_UINT32(6, reply.uintAt("v"));
	TEST_ASSERT_EQUAL_UINT32((uint32_t) ourId, reply.uintAt("id"));
	TEST_ASSERT_EQUAL_STRING("handoff", reply.stringAt("src"));
	TEST_ASSERT_EQUAL_UINT32(ourSerial, reply.uintAt("s"));
	TEST_ASSERT_EQUAL_UINT32(config::appBase, reply.uintAt("base"));
	TEST_ASSERT_EQUAL_UINT32(config::appCap, reply.uintAt("cap"));
	TEST_ASSERT_EQUAL_UINT32(config::chunkMax, reply.uintAt("chunk"));

	uint8_t uidLength = 0;
	const uint8_t * uid = reply.binaryAt("uid", uidLength);
	TEST_ASSERT_EQUAL_UINT8(12, uidLength);
	uint32_t ours[3];
	hw::uid(ours);
	TEST_ASSERT_EQUAL_UINT8_ARRAY((const uint8_t *) ours, uid, 12);

	// The installed application is reported from its own descriptor, so a host can see that a
	// board is still running a legacy-base image without having to infer it.
	TEST_ASSERT_TRUE(reply.has("app"));
	TEST_ASSERT_EQUAL_STRING("Portal v2026-08-25", reply.stringAt("ver"));
}

void test_status_answers_a_selector_addressed_broadcast()
{
	installIdentity(ourSerial);
	Bootloader bootloader(stream);
	bootloader.begin(now);

	// The case the selector exists for: the host does not know this board's id, because the board
	// power-cycled and had no application to tell its bootloader one.
	bltest::FrameBuilder builder(-1, 0, 2);
	bltest::controlBegin(builder, "status", 1);
	bltest::controlUint(builder, "s", ourSerial);
	builder.finish();
	send(builder);
	settle(bootloader);

	const bltest::Reply reply = lastReply();
	TEST_ASSERT_TRUE(reply.present);
	TEST_ASSERT_EQUAL_STRING("status", reply.verb);
}

void test_an_unselected_broadcast_status_is_never_answered()
{
	Bootloader bootloader(stream);
	bootloader.begin(now);

	bltest::FrameBuilder builder(-1, 0, 2);
	bltest::controlBegin(builder, "status", 0);
	builder.finish();
	send(builder);
	settle(bootloader);

	// Fifty-four boards answering one frame on a half-duplex bus is a collision, not a
	// conversation.
	TEST_ASSERT_EQUAL_size_t(0, stream.replyCount());
}

// ---- A whole upload -------------------------------------------------------------------------------

namespace {
	/// Drive a complete v6 upload and return the bootloader's `run` reply.
	bltest::Reply uploadImage(Bootloader & bootloader, const uint8_t * image, uint32_t length,
		uint32_t chunk, bool shuffle)
	{
		// begin
		{
			bltest::FrameBuilder builder(ourId, 0, 1);
			bltest::controlBegin(builder, "begin", 3);
			bltest::controlUint(builder, "len", length);
			bltest::controlUint(builder, "crc", portal_crc32c(image, length));
			bltest::controlUint(builder, "chunk", chunk);
			builder.finish();
			send(builder);
		}
		settle(bootloader);
		const bltest::Reply began = lastReply();
		TEST_ASSERT_TRUE_MESSAGE(began.present, "begin was not answered");
		TEST_ASSERT_EQUAL_STRING("begin", began.verb);
		TEST_ASSERT_TRUE_MESSAGE(began.boolAt("ok"), "begin refused");

		// data, one frame per chunk, optionally out of order
		uint32_t count = 0;
		for(uint32_t offset = 0; offset < length; offset += chunk) {
			count++;
		}
		for(uint32_t step = 0; step < count; step++) {
			// A stride coprime with the count visits every chunk exactly once in a scrambled
			// order -- the arrival pattern a repaired upload actually produces.
			const uint32_t index = shuffle ? ((step * 7u + 3u) % count) : step;
			const uint32_t offset = index * chunk;
			const uint32_t take = (offset + chunk > length) ? (length - offset) : chunk;

			bltest::FrameBuilder builder(-1, 0, (int) (step & 0x7F));
			bltest::dataFrame(builder, offset, image + offset, take);
			send(builder);
			settle(bootloader);
		}

		// run
		{
			bltest::FrameBuilder builder(ourId, 0, 5);
			bltest::controlBegin(builder, "run", 0);
			builder.finish();
			send(builder);
		}
		settle(bootloader);
		return lastReply();
	}

	void makeImage(uint8_t * image, uint32_t length, uint32_t base, bool descriptor)
	{
		for(uint32_t index = 0; index < length; index++) {
			image[index] = (uint8_t) (index * 31u + 7u);
		}
		const uint32_t stackPointer = PORTAL_RAM_END;
		const uint32_t resetVector = (base + 0x241u) | 1u;
		memcpy(image, &stackPointer, 4);
		memcpy(image + 4, &resetVector, 4);
		if(descriptor) {
			portal_app_descriptor_t block;
			memset(&block, 0, sizeof(block));
			memcpy(block.magic, PORTAL_APP_DESCRIPTOR_MAGIC, 8);
			block.app_base = base;
			memcpy(block.version, "Portal test", 11);
			memcpy(image + PORTAL_APP_DESCRIPTOR_OFFSET, &block, sizeof(block));
		}
	}

	uint8_t g_image[4096];
}

void test_a_complete_upload_lands_and_runs()
{
	installHandoff(ourId, ourSerial, PORTAL_HANDOFF_REQUEST_STAY);
	Bootloader bootloader(stream);
	bootloader.begin(now);

	makeImage(g_image, sizeof(g_image), config::appBase, true);
	const bltest::Reply ran = uploadImage(bootloader, g_image, sizeof(g_image), 128, false);

	TEST_ASSERT_TRUE(ran.present);
	TEST_ASSERT_EQUAL_STRING("run", ran.verb);
	TEST_ASSERT_TRUE_MESSAGE(ran.boolAt("ok"), "run refused a good image");
	TEST_ASSERT_EQUAL_UINT32(config::appBase, ran.uintAt("base"));

	TEST_ASSERT_EQUAL_UINT8_ARRAY(g_image, bltest::flashAt(config::appBase), sizeof(g_image));
	TEST_ASSERT_TRUE(bltest::terminal().ran);
	TEST_ASSERT_EQUAL_UINT32(config::appBase, bltest::terminal().base);
	// The reply is on the wire before the board goes anywhere.
	TEST_ASSERT_GREATER_THAN_UINT32(0, bltest::terminal().drains);
}

void test_an_upload_arriving_out_of_order_lands_identically()
{
	installHandoff(ourId, ourSerial, PORTAL_HANDOFF_REQUEST_STAY);
	Bootloader bootloader(stream);
	bootloader.begin(now);

	makeImage(g_image, sizeof(g_image), config::appBase, true);
	// The fielded bootloader required strictly increasing offsets and refused everything after the
	// first gap -- which is why one lost frame silently ended an upload the host thought had
	// succeeded.
	const bltest::Reply ran = uploadImage(bootloader, g_image, sizeof(g_image), 128, true);

	TEST_ASSERT_TRUE(ran.boolAt("ok"));
	TEST_ASSERT_EQUAL_UINT8_ARRAY(g_image, bltest::flashAt(config::appBase), sizeof(g_image));
}

void test_the_durable_pages_survive_a_whole_update()
{
	installIdentity(ourSerial);
	installHandoff(ourId, ourSerial, PORTAL_HANDOFF_REQUEST_STAY);

	uint8_t identityBefore[PORTAL_FLASH_PAGE_BYTES];
	memcpy(identityBefore, bltest::flashAt(PORTAL_PERSIST_IDENTITY), sizeof(identityBefore));
	const uint8_t settings[] = {'s', 'e', 't', 't', 'i', 'n', 'g', 's'};
	bltest::preload(PORTAL_PERSIST_SETTINGS_A, settings, sizeof(settings));
	bltest::preload(PORTAL_PERSIST_SETTINGS_B, settings, sizeof(settings));

	Bootloader bootloader(stream);
	bootloader.begin(now);
	makeImage(g_image, sizeof(g_image), config::appBase, true);
	uploadImage(bootloader, g_image, sizeof(g_image), 128, false);

	// The whole reason this firmware was rewritten. The fielded v4 bootloader erased three pages
	// past the application bank on every `ER`, taking the board's serial number and settings with
	// it -- and told nobody.
	TEST_ASSERT_EQUAL_UINT8_ARRAY(identityBefore, bltest::flashAt(PORTAL_PERSIST_IDENTITY),
		sizeof(identityBefore));
	TEST_ASSERT_EQUAL_UINT8_ARRAY(settings, bltest::flashAt(PORTAL_PERSIST_SETTINGS_A),
		sizeof(settings));
	TEST_ASSERT_EQUAL_UINT8_ARRAY(settings, bltest::flashAt(PORTAL_PERSIST_SETTINGS_B),
		sizeof(settings));

	// And the serial is still readable afterwards, which is the thing that actually matters.
	Bootloader after(stream);
	after.begin(now);
	bltest::FrameBuilder builder(ourId, 0, 1);
	bltest::controlBegin(builder, "status", 0);
	builder.finish();
	send(builder);
	settle(after);
	TEST_ASSERT_EQUAL_UINT32(ourSerial, lastReply().uintAt("s"));
}

// ---- map, verify, repair -----------------------------------------------------------------------

void test_map_names_exactly_the_chunks_that_went_missing()
{
	installHandoff(ourId, ourSerial, PORTAL_HANDOFF_REQUEST_STAY);
	Bootloader bootloader(stream);
	bootloader.begin(now);

	constexpr uint32_t length = 1024;
	constexpr uint32_t chunk = 128;
	makeImage(g_image, length, config::appBase, true);

	{
		bltest::FrameBuilder builder(ourId, 0, 1);
		bltest::controlBegin(builder, "begin", 3);
		bltest::controlUint(builder, "len", length);
		bltest::controlUint(builder, "crc", portal_crc32c(g_image, length));
		bltest::controlUint(builder, "chunk", chunk);
		builder.finish();
		send(builder);
	}
	settle(bootloader);
	lastReply();

	// Send every chunk except 2 and 5, as a lossy bus would.
	for(uint32_t index = 0; index < length / chunk; index++) {
		if(index == 2 || index == 5) {
			continue;
		}
		bltest::FrameBuilder builder(-1, 0, -1);
		bltest::dataFrame(builder, index * chunk, g_image + index * chunk, chunk);
		send(builder);
		settle(bootloader);
	}

	{
		bltest::FrameBuilder builder(ourId, 0, 3);
		bltest::controlBegin(builder, "map", 0);
		builder.finish();
		send(builder);
	}
	settle(bootloader);

	const bltest::Reply mapped = lastReply();
	TEST_ASSERT_EQUAL_STRING("map", mapped.verb);
	TEST_ASSERT_EQUAL_UINT32(chunk, mapped.uintAt("chunk"));
	TEST_ASSERT_EQUAL_UINT32(length, mapped.uintAt("len"));
	uint8_t bits = 0;
	uint8_t bitmapLength = 0;
	const uint8_t * bitmap = mapped.binaryAt("map", bitmapLength);
	TEST_ASSERT_EQUAL_UINT8(1, bitmapLength);
	bits = bitmap[0];
	// Chunks 0,1,3,4,6,7 arrived: 0b1101'1011. This is what lets the host repair exactly the gaps
	// instead of sending the whole image again and hoping.
	TEST_ASSERT_EQUAL_UINT8(0b1101'1011, bits);

	// verify must refuse an incomplete image, and run must refuse it too.
	{
		bltest::FrameBuilder builder(ourId, 0, 4);
		bltest::controlBegin(builder, "verify", 0);
		builder.finish();
		send(builder);
	}
	settle(bootloader);
	TEST_ASSERT_FALSE(lastReply().boolAt("ok"));

	{
		bltest::FrameBuilder builder(ourId, 0, 5);
		bltest::controlBegin(builder, "run", 0);
		builder.finish();
		send(builder);
	}
	settle(bootloader);
	const bltest::Reply refused = lastReply();
	TEST_ASSERT_FALSE(refused.boolAt("ok"));
	TEST_ASSERT_EQUAL_UINT32((uint32_t) code(Error::ImageCrc), refused.uintAt("err"));
	TEST_ASSERT_FALSE(bltest::terminal().ran);

	// Repair the two gaps, and now everything agrees.
	for(uint32_t index : {2u, 5u}) {
		bltest::FrameBuilder builder(-1, 0, -1);
		bltest::dataFrame(builder, index * chunk, g_image + index * chunk, chunk);
		send(builder);
		settle(bootloader);
	}
	{
		bltest::FrameBuilder builder(ourId, 0, 6);
		bltest::controlBegin(builder, "map", 0);
		builder.finish();
		send(builder);
	}
	settle(bootloader);
	const bltest::Reply repaired = lastReply();
	repaired.binaryAt("map", bitmapLength);
	TEST_ASSERT_EQUAL_UINT8(0xFF, repaired.binaryAt("map", bitmapLength)[0]);

	{
		bltest::FrameBuilder builder(ourId, 0, 7);
		bltest::controlBegin(builder, "verify", 0);
		builder.finish();
		send(builder);
	}
	settle(bootloader);
	const bltest::Reply verified = lastReply();
	TEST_ASSERT_TRUE(verified.boolAt("ok"));
	TEST_ASSERT_EQUAL_UINT32(portal_crc32c(g_image, length), verified.uintAt("crc"));
}

// ---- Refusals ------------------------------------------------------------------------------------

void test_run_refuses_an_image_with_no_descriptor()
{
	installHandoff(ourId, ourSerial, PORTAL_HANDOFF_REQUEST_STAY);
	Bootloader bootloader(stream);
	bootloader.begin(now);

	// A legacy-base image delivered to the new base: plausible vector table, every absolute
	// address inside it 8 kB wrong.
	makeImage(g_image, sizeof(g_image), config::appBase, false);
	const bltest::Reply ran = uploadImage(bootloader, g_image, sizeof(g_image), 128, false);

	TEST_ASSERT_FALSE(ran.boolAt("ok"));
	TEST_ASSERT_EQUAL_UINT32((uint32_t) code(Error::DescriptorMissing), ran.uintAt("err"));
	TEST_ASSERT_FALSE(bltest::terminal().ran);
	// And it stays resident rather than resetting into the same decision.
	TEST_ASSERT_TRUE(bootloader.indefinite());
	TEST_ASSERT_EQUAL(Phase::Held, bootloader.phase());
}

void test_run_refuses_an_image_built_for_the_other_bank()
{
	installHandoff(ourId, ourSerial, PORTAL_HANDOFF_REQUEST_STAY);
	Bootloader bootloader(stream);
	bootloader.begin(now);

	makeImage(g_image, sizeof(g_image), config::appBaseLegacy, true);
	const bltest::Reply ran = uploadImage(bootloader, g_image, sizeof(g_image), 128, false);

	TEST_ASSERT_FALSE(ran.boolAt("ok"));
	TEST_ASSERT_EQUAL_UINT32((uint32_t) code(Error::DescriptorBase), ran.uintAt("err"));
	TEST_ASSERT_FALSE(bltest::terminal().ran);
}

void test_a_board_with_nothing_to_run_waits_indefinitely()
{
	Bootloader bootloader(stream);
	bootloader.begin(now);

	// Run the countdown out.
	now += config::residencyDefault + 1;
	bootloader.tick(now);

	TEST_ASSERT_FALSE(bltest::terminal().ran);
	TEST_ASSERT_TRUE(bootloader.indefinite());
	TEST_ASSERT_EQUAL(Error::NoApp, bootloader.lastError());

	// A board that cannot run is one a host most needs to reach; a reset loop would look exactly
	// like dead hardware from the outside. It is still answering.
	installIdentity(ourSerial);
	Bootloader second(stream);
	second.begin(now);
	now += 100000;
	second.tick(now);
	bltest::FrameBuilder builder(-1, 0, 1);
	bltest::controlBegin(builder, "status", 1);
	bltest::controlUint(builder, "s", ourSerial);
	builder.finish();
	send(builder);
	settle(second);
	TEST_ASSERT_TRUE(lastReply().present);
}

// ---- The legacy flow ------------------------------------------------------------------------------

void test_a_legacy_host_can_still_drive_a_complete_update()
{
	// No handoff, no control plane, no trailer: an un-updated Router doing exactly what it does
	// today. It must keep working, because on the day this firmware first runs, every host is one.
	Bootloader bootloader(stream);
	bootloader.begin(now);

	constexpr uint32_t length = 1024;
	makeImage(g_image, length, config::appBaseLegacy, false);

	{
		bltest::FrameBuilder builder(-1, 0, -1);
		bltest::magicFrame(builder, "FW");
		send(builder);
	}
	settle(bootloader);

	{
		bltest::FrameBuilder builder(-1, 0, -1);
		bltest::magicFrame(builder, "ER");
		send(builder);
	}
	settle(bootloader);

	for(uint32_t offset = 0; offset < length; offset += 32) {
		bltest::FrameBuilder builder(-1, 0, -1);
		bltest::dataFrame(builder, offset, g_image + offset, 32);
		send(builder);
		settle(bootloader);
	}

	{
		bltest::FrameBuilder builder(-1, 0, -1);
		bltest::magicFrame(builder, "RU");
		send(builder);
	}
	settle(bootloader);

	// A legacy `ER` writes at the legacy base, because a host old enough to send that word is
	// sending an image linked for 0x08006000.
	TEST_ASSERT_EQUAL_UINT8_ARRAY(g_image, bltest::flashAt(config::appBaseLegacy), length);
	TEST_ASSERT_TRUE(bltest::terminal().ran);
	TEST_ASSERT_EQUAL_UINT32(config::appBaseLegacy, bltest::terminal().base);
}

void test_a_legacy_announce_does_not_discard_an_open_session()
{
	installHandoff(ourId, ourSerial, PORTAL_HANDOFF_REQUEST_STAY);
	Bootloader bootloader(stream);
	bootloader.begin(now);

	constexpr uint32_t length = 512;
	makeImage(g_image, length, config::appBase, true);

	{
		bltest::FrameBuilder builder(ourId, 0, 1);
		bltest::controlBegin(builder, "begin", 3);
		bltest::controlUint(builder, "len", length);
		bltest::controlUint(builder, "crc", portal_crc32c(g_image, length));
		bltest::controlUint(builder, "chunk", 128);
		builder.finish();
		send(builder);
	}
	settle(bootloader);
	lastReply();

	{
		bltest::FrameBuilder builder(-1, 0, -1);
		bltest::dataFrame(builder, 0, g_image, 128);
		send(builder);
	}
	settle(bootloader);

	// A host holding legacy bootloaders resident keeps sending "FW" throughout. The v4/v5
	// bootloader treated that word as "reset the write position", so the announce frames sent to
	// cover somebody else's erase would silently discard this board's progress.
	{
		bltest::FrameBuilder builder(-1, 0, -1);
		bltest::magicFrame(builder, "FW");
		send(builder);
	}
	settle(bootloader);

	TEST_ASSERT_EQUAL_UINT32(128, bootloader.session().received());
	TEST_ASSERT_EQUAL_UINT32(128, bootloader.session().highWater());
}

// ---- Timers ----------------------------------------------------------------------------------------

void test_an_accepted_frame_pushes_the_deadline_out()
{
	Bootloader bootloader(stream);
	bootloader.begin(now);
	const uint32_t original = bootloader.deadline();

	now += 1000;
	bltest::FrameBuilder builder(-1, 0, -1);
	bltest::magicFrame(builder, "FW");
	send(builder);
	bootloader.tick(now);

	TEST_ASSERT_GREATER_THAN_UINT32(original, bootloader.deadline());
	TEST_ASSERT_EQUAL_UINT32(now + config::residencyExtend, bootloader.deadline());
}

void test_a_short_extension_never_pulls_a_deadline_closer()
{
	installHandoff(ourId, ourSerial, PORTAL_HANDOFF_REQUEST_STAY);
	Bootloader bootloader(stream);
	bootloader.begin(now);
	const uint32_t original = bootloader.deadline();
	TEST_ASSERT_EQUAL_UINT32(now + config::residencyHandoff, original);

	// A ten-second extension arriving inside a thirty-second stay must not cut it short.
	bltest::FrameBuilder builder(-1, 0, -1);
	bltest::magicFrame(builder, "FW");
	send(builder);
	bootloader.tick(now);
	TEST_ASSERT_EQUAL_UINT32(original, bootloader.deadline());
}

void test_reset_replies_before_it_resets()
{
	installHandoff(ourId, ourSerial, PORTAL_HANDOFF_REQUEST_STAY);
	Bootloader bootloader(stream);
	bootloader.begin(now);

	bltest::FrameBuilder builder(ourId, 0, 8);
	bltest::controlBegin(builder, "reset", 0);
	builder.finish();
	send(builder);
	bootloader.tick(now);

	const bltest::Reply reply = lastReply();
	TEST_ASSERT_TRUE(reply.present);
	TEST_ASSERT_EQUAL_STRING("reset", reply.verb);
	TEST_ASSERT_TRUE(reply.boolAt("ok"));
	TEST_ASSERT_TRUE(bltest::terminal().reset);
	// Drained first: the driver-enable line follows the transmitter, so resetting a microsecond
	// early truncates the reply and loses the bus turnaround with it.
	TEST_ASSERT_GREATER_THAN_UINT32(0, bltest::terminal().drains);
}

void test_adopt_sets_the_address_and_records_it()
{
	installIdentity(ourSerial);
	bltest::setDip(0);
	Bootloader bootloader(stream);
	bootloader.begin(now);
	TEST_ASSERT_EQUAL_INT8(1, bootloader.address());

	bltest::FrameBuilder builder(-1, 0, 4);
	bltest::controlBegin(builder, "adopt", 2);
	bltest::controlUint(builder, "s", ourSerial);
	bltest::controlInt(builder, "id", 42);
	builder.finish();
	send(builder);
	settle(bootloader);

	const bltest::Reply reply = lastReply();
	TEST_ASSERT_EQUAL_STRING("adopt", reply.verb);
	TEST_ASSERT_EQUAL_INT8(42, bootloader.address());
	TEST_ASSERT_EQUAL(IdSource::Adopt, bootloader.idSource());
	// Recorded in the handoff block, so a watchdog reset does not lose it.
	TEST_ASSERT_EQUAL_INT8(42, hw::handoff()->id);

	// And it now answers on that address.
	bltest::FrameBuilder next(42, 0, 5);
	bltest::controlBegin(next, "status", 0);
	next.finish();
	send(next);
	settle(bootloader);
	TEST_ASSERT_TRUE(lastReply().present);
}

// ---- Runner ---------------------------------------------------------------------------------------

int main()
{
	UNITY_BEGIN();

	RUN_TEST(test_the_address_comes_from_the_application_when_it_left_one);
	RUN_TEST(test_the_request_is_consumed_but_the_identity_is_kept);
	RUN_TEST(test_a_corrupt_handoff_falls_back_to_the_switches);

	RUN_TEST(test_status_reports_the_board);
	RUN_TEST(test_status_answers_a_selector_addressed_broadcast);
	RUN_TEST(test_an_unselected_broadcast_status_is_never_answered);

	RUN_TEST(test_a_complete_upload_lands_and_runs);
	RUN_TEST(test_an_upload_arriving_out_of_order_lands_identically);
	RUN_TEST(test_the_durable_pages_survive_a_whole_update);

	RUN_TEST(test_map_names_exactly_the_chunks_that_went_missing);

	RUN_TEST(test_run_refuses_an_image_with_no_descriptor);
	RUN_TEST(test_run_refuses_an_image_built_for_the_other_bank);
	RUN_TEST(test_a_board_with_nothing_to_run_waits_indefinitely);

	RUN_TEST(test_a_legacy_host_can_still_drive_a_complete_update);
	RUN_TEST(test_a_legacy_announce_does_not_discard_an_open_session);

	RUN_TEST(test_an_accepted_frame_pushes_the_deadline_out);
	RUN_TEST(test_a_short_extension_never_pulls_a_deadline_closer);
	RUN_TEST(test_reset_replies_before_it_resets);
	RUN_TEST(test_adopt_sets_the_address_and_records_it);

	return UNITY_END();
}
