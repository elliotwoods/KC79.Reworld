// Parsing frames off the bus: what is accepted, what is ignored, and what is rejected.
//
// Every frame here is built by the *real* `COBSRWStream` and msgpack serializer -- the same code
// the host and the application use -- so a pass is evidence about the codec that ships rather than
// about a re-implementation of it that happens to agree with itself.
//
// The three outcomes are deliberately distinct and are not interchangeable:
//
//   ignored   not addressed to us, or not something a bootloader has an opinion about. No reply,
//             no error, no trace. A unicast poll to a board running its application must not draw
//             a reply from a board sitting in its bootloader.
//   rejected  addressed to us and wrong: corrupt, malformed, out of bounds. Recorded, and visible
//             in `status.err`.
//   accepted  acted upon.

#include <unity.h>

#include "bl/link.hpp"
#include "fake_hw.hpp"
#include "frames.hpp"

#include <initializer_list>
#include <stdio.h>
#include <string.h>

using namespace bl;

namespace {
	constexpr int8_t ourId = 3;
	constexpr uint32_t ourSerial = 73001;
	const uint32_t ourUid[3] = {0x1122'3344u, 0x5566'7788u, 0x99AA'BBCCu};

	bltest::DuplexStream stream;

	Link makeLink()
	{
		Link link(stream);
		link.setAddress(ourId);
		link.setIdentity(ourSerial, ourUid);
		return link;
	}

	void deliver(bltest::FrameBuilder & builder)
	{
		stream.deliver(builder.bytes(), builder.size());
	}
}

void setUp()
{
	bltest::reset();
	stream = bltest::DuplexStream();
}

void tearDown() {}

// ---- Addressing ---------------------------------------------------------------------------------

void test_a_broadcast_announce_is_accepted_without_a_reply()
{
	bltest::FrameBuilder builder(-1, 0, -1);
	bltest::magicFrame(builder, "FW");
	deliver(builder);

	Link link = makeLink();
	const Command command = link.receive();
	TEST_ASSERT_EQUAL(CommandKind::Announce, command.kind);
	TEST_ASSERT_FALSE(command.replyAllowed);
}

void test_the_long_announce_word_is_also_an_announce()
{
	// The fielded v4/v5 bootloader parsed an announce into a 3-byte buffer, so the 7-byte
	// "FW!KC79" the application listens for was a *format error* to it. That is the entire reason
	// the host has to interleave the two words rather than simply sending the long one; accepting
	// the prefix costs nothing and lets a future host stop caring.
	bltest::FrameBuilder builder(-1, 0, -1);
	bltest::magicFrame(builder, "FW!KC79");
	deliver(builder);

	Link link = makeLink();
	TEST_ASSERT_EQUAL(CommandKind::Announce, link.receive().kind);
}

void test_erase_and_run_words_are_recognised()
{
	for(const char * word : {"ER", "RU"}) {
		bltest::reset();
		stream = bltest::DuplexStream();
		bltest::FrameBuilder builder(-1, 0, -1);
		bltest::magicFrame(builder, word);
		deliver(builder);

		Link link = makeLink();
		const Command command = link.receive();
		TEST_ASSERT_TRUE(command.kind == CommandKind::Erase || command.kind == CommandKind::RunLegacy);
	}
}

void test_a_unicast_to_us_may_be_answered()
{
	bltest::FrameBuilder builder(ourId, 0, 7);
	bltest::controlBegin(builder, "status", 0);
	builder.finish();
	deliver(builder);

	Link link = makeLink();
	const Command command = link.receive();
	TEST_ASSERT_EQUAL(CommandKind::Control, command.kind);
	TEST_ASSERT_EQUAL(Verb::Status, command.verb);
	TEST_ASSERT_TRUE(command.replyAllowed);
	TEST_ASSERT_EQUAL_UINT8(7, command.seq);
}

void test_a_unicast_to_another_board_is_ignored_entirely()
{
	bltest::FrameBuilder builder(ourId + 1, 0, 7);
	bltest::controlBegin(builder, "status", 0);
	builder.finish();
	deliver(builder);

	Link link = makeLink();
	const Command command = link.receive();
	TEST_ASSERT_EQUAL(CommandKind::None, command.kind);
	TEST_ASSERT_EQUAL(Error::None, command.error);
}

void test_ordinary_application_traffic_draws_no_response()
{
	// A position poll addressed to us. The application would answer it; a bootloader has nothing
	// to say, and saying nothing is what keeps a board in its bootloader from being mistaken for
	// a board running its application.
	bltest::FrameBuilder builder(ourId, 0, -1);
	msgpack::writeMapSize4(builder.body(), 1);
	msgpack::writeString5(builder.body(), "p", 1);
	msgpack::writeNil(builder.body());
	builder.finish();
	deliver(builder);

	Link link = makeLink();
	const Command command = link.receive();
	TEST_ASSERT_EQUAL(CommandKind::None, command.kind);
	TEST_ASSERT_EQUAL_size_t(0, stream.replyCount());
}

// ---- Selectors ------------------------------------------------------------------------------------

void test_a_broadcast_with_our_serial_may_be_answered()
{
	// The escape hatch that matters: a board that power-cycled has no application to tell its
	// bootloader what its RS485 id is, so the host addresses it by the serial in its identity page.
	bltest::FrameBuilder builder(-1, 0, 9);
	bltest::controlBegin(builder, "status", 1);
	bltest::controlUint(builder, "s", ourSerial);
	builder.finish();
	deliver(builder);

	Link link = makeLink();
	const Command command = link.receive();
	TEST_ASSERT_EQUAL(CommandKind::Control, command.kind);
	TEST_ASSERT_TRUE(command.replyAllowed);
}

void test_a_broadcast_naming_another_board_is_ignored()
{
	bltest::FrameBuilder builder(-1, 0, 9);
	bltest::controlBegin(builder, "status", 1);
	bltest::controlUint(builder, "s", ourSerial + 1);
	builder.finish();
	deliver(builder);

	Link link = makeLink();
	const Command command = link.receive();
	// Not ours to answer *or* act on. Acting would erase the wrong board's flash.
	TEST_ASSERT_EQUAL(CommandKind::None, command.kind);
}

void test_a_board_with_no_identity_cannot_be_selected_by_serial()
{
	bltest::FrameBuilder builder(-1, 0, 9);
	bltest::controlBegin(builder, "status", 1);
	bltest::controlUint(builder, "s", 0);
	builder.finish();
	deliver(builder);

	Link link(stream);
	link.setAddress(ourId);
	link.setIdentity(0, ourUid); // no valid identity record
	// Serial 0 is never a valid serial. If it selected, a rack of unprovisioned boards would all
	// answer the same frame at once.
	TEST_ASSERT_EQUAL(CommandKind::None, link.receive().kind);
}

void test_a_broadcast_with_our_uid_may_be_answered()
{
	uint8_t uidBytes[12];
	memcpy(uidBytes, ourUid, sizeof(uidBytes));

	bltest::FrameBuilder builder(-1, 0, 9);
	bltest::controlBegin(builder, "status", 1);
	bltest::controlBinary(builder, "uid", uidBytes, sizeof(uidBytes));
	builder.finish();
	deliver(builder);

	Link link = makeLink();
	const Command command = link.receive();
	TEST_ASSERT_EQUAL(CommandKind::Control, command.kind);
	TEST_ASSERT_TRUE(command.replyAllowed);
}

void test_an_unselected_broadcast_acts_but_does_not_answer()
{
	bltest::FrameBuilder builder(-1, 0, 4);
	bltest::controlBegin(builder, "begin", 3);
	bltest::controlUint(builder, "len", 1024);
	bltest::controlUint(builder, "crc", 0xDEADBEEF);
	bltest::controlUint(builder, "chunk", 128);
	builder.finish();
	deliver(builder);

	Link link = makeLink();
	const Command command = link.receive();
	// This is what lets one `begin` open a session on 54 boards at once.
	TEST_ASSERT_EQUAL(CommandKind::Control, command.kind);
	TEST_ASSERT_EQUAL(Verb::Begin, command.verb);
	TEST_ASSERT_FALSE(command.replyAllowed);
}

void test_an_unselected_broadcast_adopt_is_refused()
{
	bltest::FrameBuilder builder(-1, 0, 4);
	bltest::controlBegin(builder, "adopt", 1);
	bltest::controlInt(builder, "id", 5);
	builder.finish();
	deliver(builder);

	Link link = makeLink();
	// Every board on the bus adopting the same id is how a bus becomes unusable.
	TEST_ASSERT_EQUAL(CommandKind::None, link.receive().kind);
}

// ---- The trailer ------------------------------------------------------------------------------------

void test_a_legacy_three_element_frame_is_accepted()
{
	// The fielded Router sends nothing else. Refusing it would break every host that has not been
	// updated, which is all of them at the moment this firmware first runs.
	bltest::FrameBuilder builder(-1, 0, -1);
	bltest::magicFrame(builder, "FW");
	deliver(builder);

	Link link = makeLink();
	const Command command = link.receive();
	TEST_ASSERT_EQUAL(CommandKind::Announce, command.kind);
	TEST_ASSERT_EQUAL_UINT8(0, command.seq);
}

void test_a_good_trailer_is_verified_and_its_sequence_kept()
{
	bltest::FrameBuilder builder(ourId, 0, 42);
	bltest::controlBegin(builder, "status", 0);
	builder.finish();
	deliver(builder);

	Link link = makeLink();
	const Command command = link.receive();
	TEST_ASSERT_EQUAL(CommandKind::Control, command.kind);
	TEST_ASSERT_EQUAL_UINT8(42, command.seq);
}

void test_a_corrupted_frame_is_rejected_rather_than_acted_on()
{
	bltest::FrameBuilder builder(ourId, 0, 42);
	bltest::controlBegin(builder, "begin", 3);
	bltest::controlUint(builder, "len", 1024);
	bltest::controlUint(builder, "crc", 0xDEADBEEF);
	bltest::controlUint(builder, "chunk", 128);
	builder.finish();
	// One flipped bit in the middle of the body. Without the trailer this would decode cleanly as
	// a `begin` with a different length, and erase a bank on the strength of it.
	builder.corrupt(builder.size() / 2);
	deliver(builder);

	Link link = makeLink();
	const Command command = link.receive();
	TEST_ASSERT_EQUAL(CommandKind::Rejected, command.kind);
	TEST_ASSERT_EQUAL(Error::Crc16, command.error);
	TEST_ASSERT_FALSE(command.replyAllowed);
}

void test_no_corrupted_frame_is_ever_acted_on_with_altered_content()
{
	// Every byte of the frame, not a sample: the trailer's worth is that it covers the addresses
	// and the map structure as well as the payload, and a check that only covered part of the
	// frame would still pass a test that only corrupted that part.
	//
	// The assertion is not "everything is rejected", because that is not true and pretending it
	// were would hide why. A COBS *code* byte can sometimes be corrupted into one that decodes to
	// exactly the same bytes -- raising the final chunk's length code, for instance, when the
	// delimiter ends the chunk before the extra byte is reached. The frame's content is then
	// genuinely unaltered and acting on it is correct. What must never happen is a frame being
	// acted on with content that differs from what was sent.
	size_t accepted = 0;
	size_t rejected = 0;

	for(size_t index = 1;; index++) {
		bltest::reset();
		stream = bltest::DuplexStream();

		bltest::FrameBuilder builder(ourId, 0, 42);
		bltest::controlBegin(builder, "begin", 3);
		bltest::controlUint(builder, "len", 4096);
		bltest::controlUint(builder, "crc", 0x1234'5678);
		bltest::controlUint(builder, "chunk", 128);
		builder.finish();
		if(index >= builder.size()) {
			break;
		}
		builder.corrupt(index);
		deliver(builder);

		Link link = makeLink();
		const Command command = link.receive();

		if(command.kind == CommandKind::Control) {
			accepted++;
			TEST_ASSERT_EQUAL_MESSAGE(Verb::Begin, command.verb, "a corrupted frame changed verb");
			TEST_ASSERT_EQUAL_UINT32_MESSAGE(4096, command.length, "a corrupted frame changed len");
			TEST_ASSERT_EQUAL_UINT32_MESSAGE(0x1234'5678, command.crc, "a corrupted frame changed crc");
			TEST_ASSERT_EQUAL_UINT32_MESSAGE(128, command.chunk, "a corrupted frame changed chunk");
		}
		else {
			rejected++;
		}
	}

	// And the check is doing real work: the overwhelming majority are caught outright.
	TEST_ASSERT_GREATER_THAN_size_t(accepted * 4, rejected);
}

// ---- Data frames ------------------------------------------------------------------------------------

void test_a_data_frame_carries_its_offset_and_payload()
{
	uint8_t payload[64];
	for(uint8_t index = 0; index < sizeof(payload); index++) {
		payload[index] = (uint8_t) (index * 3);
	}

	bltest::FrameBuilder builder(-1, 0, -1);
	bltest::dataFrame(builder, 512, payload, sizeof(payload));
	deliver(builder);

	Link link = makeLink();
	const Command command = link.receive();
	TEST_ASSERT_EQUAL(CommandKind::Data, command.kind);
	TEST_ASSERT_EQUAL_UINT32(512, command.offset);
	TEST_ASSERT_EQUAL_UINT32(sizeof(payload), command.payloadLength);
	TEST_ASSERT_EQUAL_UINT8_ARRAY(payload, command.payload, sizeof(payload));
}

void test_a_data_frame_with_a_bad_payload_checksum_is_rejected()
{
	uint8_t payload[32] = {};
	bltest::FrameBuilder builder(-1, 0, -1);
	bltest::dataFrame(builder, 0, payload, sizeof(payload), true);
	deliver(builder);

	Link link = makeLink();
	const Command command = link.receive();
	TEST_ASSERT_EQUAL(CommandKind::Rejected, command.kind);
	TEST_ASSERT_EQUAL(Error::Xor, command.error);
}

void test_a_large_offset_survives_the_full_bank()
{
	// The offset is a uint32 the whole way. The folklore that a 16-bit field limited image size
	// was traced to a different bug entirely, but the boundary is worth pinning: the last chunk
	// of a full v6 bank starts at 108,288, well past 65,535.
	uint8_t payload[128] = {};
	const uint32_t lastOffset = config::appCap - sizeof(payload);

	bltest::FrameBuilder builder(-1, 0, -1);
	bltest::dataFrame(builder, lastOffset, payload, sizeof(payload));
	deliver(builder);

	Link link = makeLink();
	const Command command = link.receive();
	TEST_ASSERT_EQUAL(CommandKind::Data, command.kind);
	TEST_ASSERT_EQUAL_UINT32(lastOffset, command.offset);
	TEST_ASSERT_GREATER_THAN_UINT32(65535, lastOffset);
}

void test_every_msgpack_offset_width_decodes_to_the_same_number()
{
	// The encoder narrows an offset to the smallest type that holds it, so an upload crosses three
	// encodings on its way up the bank. Each boundary is a place a narrowing bug could hide.
	const uint32_t offsets[] = {0, 120, 128, 255, 256, 65535, 65536, 100000, config::appCap - 8};
	uint8_t payload[8] = {1, 2, 3, 4, 5, 6, 7, 8};

	for(uint32_t offset : offsets) {
		bltest::reset();
		stream = bltest::DuplexStream();

		bltest::FrameBuilder builder(-1, 0, -1);
		bltest::dataFrame(builder, offset, payload, sizeof(payload));
		deliver(builder);

		Link link = makeLink();
		const Command command = link.receive();
		TEST_ASSERT_EQUAL(CommandKind::Data, command.kind);
		TEST_ASSERT_EQUAL_UINT32(offset, command.offset);
	}
}

void test_an_oversized_payload_is_refused_before_a_byte_is_read()
{
	// The fielded bootloader sized a stack VLA from this field with no upper bound, so a frame
	// claiming 60,000 bytes would smash the stack of an image only recoverable with a debug probe.
	// The bound is checked first, against a fixed buffer, and a claim beyond it never reaches a
	// read at all.
	bltest::FrameBuilder builder(-1, 0, -1);
	msgpack::writeMapSize4(builder.body(), 1);
	msgpack::writeIntU32(builder.body(), 0);
	msgpack::writeRawByte(builder.body(), 0xC5); // bin16
	msgpack::writeRawByte(builder.body(), 0xFF);
	msgpack::writeRawByte(builder.body(), 0xFF); // 65,535 bytes claimed
	const uint8_t few[4] = {0, 0, 0xAA, 0x55};   // four actually present
	msgpack::writeRaw(builder.body(), few, sizeof(few));
	builder.finish();
	deliver(builder);

	Link link = makeLink();
	const Command command = link.receive();
	TEST_ASSERT_EQUAL(CommandKind::Rejected, command.kind);
	TEST_ASSERT_EQUAL(Error::Format, command.error);
}

void test_a_payload_shorter_than_its_own_checksum_is_refused()
{
	// Underflows the length subtraction if it is not caught: `declared - 2` on a declared 1.
	bltest::FrameBuilder builder(-1, 0, -1);
	msgpack::writeMapSize4(builder.body(), 1);
	msgpack::writeIntU32(builder.body(), 0);
	msgpack::writeRawByte(builder.body(), 0xC4);
	msgpack::writeRawByte(builder.body(), 1);
	msgpack::writeRawByte(builder.body(), 0x00);
	builder.finish();
	deliver(builder);

	Link link = makeLink();
	const Command command = link.receive();
	TEST_ASSERT_EQUAL(CommandKind::Rejected, command.kind);
	TEST_ASSERT_EQUAL(Error::Format, command.error);
}

// ---- Robustness ---------------------------------------------------------------------------------

void test_a_truncated_frame_never_blocks()
{
	// The msgpack library blocks when it runs out of bytes: `waitForData` spins for 100 ms and
	// `readBytes` spins forever. Neither is acceptable in a loop that services a watchdog, and the
	// defence is that the stream reports nothing until a whole frame has arrived. A partial frame
	// with no delimiter is simply not visible.
	bltest::FrameBuilder builder(-1, 0, -1);
	bltest::magicFrame(builder, "FW");
	stream.deliver(builder.bytes(), builder.size() - 1); // delimiter withheld

	Link link = makeLink();
	TEST_ASSERT_FALSE(link.pending());
	TEST_ASSERT_EQUAL(CommandKind::None, link.receive().kind);
}

void test_an_unknown_control_key_is_skipped_rather_than_fatal()
{
	// Forward compatibility: a host built against a later revision of this protocol adds fields,
	// and this bootloader has to keep answering rather than rejecting the frame wholesale.
	bltest::FrameBuilder builder(ourId, 0, 3);
	bltest::controlBegin(builder, "status", 2);
	bltest::controlUint(builder, "zz", 12345);
	msgpack::writeString5(builder.body(), "yy", 2);
	msgpack::writeString5(builder.body(), "later", 5);
	builder.finish();
	deliver(builder);

	Link link = makeLink();
	const Command command = link.receive();
	TEST_ASSERT_EQUAL(CommandKind::Control, command.kind);
	TEST_ASSERT_EQUAL(Verb::Status, command.verb);
}

void test_an_unknown_verb_is_reported()
{
	bltest::FrameBuilder builder(ourId, 0, 3);
	bltest::controlBegin(builder, "teleport", 0);
	builder.finish();
	deliver(builder);

	Link link = makeLink();
	const Command command = link.receive();
	TEST_ASSERT_EQUAL(CommandKind::Rejected, command.kind);
	TEST_ASSERT_EQUAL(Error::UnknownVerb, command.error);
}

void test_frames_are_consumed_one_at_a_time_and_in_order()
{
	uint8_t payload[8] = {1, 2, 3, 4, 5, 6, 7, 8};
	for(uint32_t offset = 0; offset < 24; offset += 8) {
		bltest::FrameBuilder builder(-1, 0, -1);
		bltest::dataFrame(builder, offset, payload, sizeof(payload));
		deliver(builder);
	}

	Link link = makeLink();
	for(uint32_t offset = 0; offset < 24; offset += 8) {
		const Command command = link.receive();
		TEST_ASSERT_EQUAL(CommandKind::Data, command.kind);
		TEST_ASSERT_EQUAL_UINT32(offset, command.offset);
	}
	TEST_ASSERT_EQUAL(CommandKind::None, link.receive().kind);
}

void test_the_window_holds_one_frame_and_never_blocks()
{
	// This test used to assert the opposite: that `COBSRWStream` merged back-to-back packets when
	// `available()` was called twice before the first `read()`, which it did, and which is why
	// `FrameWindow` was written. That defect is now fixed in the library
	// (`msgpack-arduino`, test_cobs_backtoback), and this test failing is how that arrived --
	// which was the point of writing it as a characterisation test rather than a comment.
	//
	// The window stays, for the property the library fix does not provide: a parser must never be
	// able to ask for a byte that has not arrived. `NotArduino`'s `readBytes` loops *forever*
	// waiting for one, with no timeout, and that is not something a loop servicing a watchdog can
	// risk on a bus where a frame can be cut in half by a disconnected cable.
	uint8_t payload[8] = {1, 2, 3, 4, 5, 6, 7, 8};

	// Half a frame, and no more. The window must show the codec nothing at all.
	{
		bltest::FrameBuilder builder(-1, 0, -1);
		bltest::dataFrame(builder, 0, payload, sizeof(payload));
		stream.deliver(builder.bytes(), builder.size() / 2);
	}
	Link link = makeLink();
	TEST_ASSERT_FALSE_MESSAGE(link.pending(), "a partial frame was exposed to the parser");
	TEST_ASSERT_EQUAL(CommandKind::None, link.receive().kind);

	// The rest of it arrives, and now it parses -- assembled across two deliveries rather than
	// dropped, which is what a UART interrupt boundary looks like from here.
	{
		bltest::reset();
		stream = bltest::DuplexStream();
		bltest::FrameBuilder builder(-1, 0, -1);
		bltest::dataFrame(builder, 0, payload, sizeof(payload));
		const size_t half = builder.size() / 2;
		stream.deliver(builder.bytes(), half);
		stream.deliver(builder.bytes() + half, builder.size() - half);
	}
	Link resumed = makeLink();
	TEST_ASSERT_TRUE(resumed.pending());
	const Command command = resumed.receive();
	TEST_ASSERT_EQUAL(CommandKind::Data, command.kind);
	TEST_ASSERT_EQUAL_UINT32(sizeof(payload), command.payloadLength);
}

void test_every_frame_offset_of_a_completely_full_bank_decodes()
{
	// All 3,392 legacy 32-byte frames of a full 108,544-byte bank, each built by the real encoder
	// and parsed by the real parser.
	//
	// Exhaustive rather than sampled because the failure this guards against is a *narrowing*, and
	// a narrowing shows up only past a boundary that a sample can miss. The folklore that image
	// size was capped by a 16-bit offset field was traced to a different bug entirely, but the
	// cheapest way to keep it settled is to walk the whole bank: the encoder crosses fixint,
	// uint8, uint16 and uint32 on the way up, and every one of those transitions is checked here
	// by construction rather than by being chosen.
	constexpr uint32_t frameBytes = 32;
	uint8_t payload[frameBytes];
	for(uint32_t index = 0; index < frameBytes; index++) {
		payload[index] = (uint8_t) (index * 7u + 1u);
	}

	uint32_t checked = 0;
	for(uint32_t offset = 0; offset < config::appCap; offset += frameBytes) {
		bltest::reset();
		stream = bltest::DuplexStream();

		bltest::FrameBuilder builder(-1, 0, -1);
		bltest::dataFrame(builder, offset, payload, frameBytes);
		deliver(builder);

		Link link = makeLink();
		const Command command = link.receive();
		if(command.kind != CommandKind::Data || command.offset != offset) {
			// One failure per run rather than 3,392: report the offset that broke and stop.
			char message[96];
			snprintf(message, sizeof(message),
				"offset %u decoded as kind=%d offset=%u", offset, (int) command.kind,
				command.offset);
			TEST_FAIL_MESSAGE(message);
		}
		checked++;
	}

	TEST_ASSERT_EQUAL_UINT32(3392, checked);
}

// ---- Runner -------------------------------------------------------------------------------------

int main()
{
	UNITY_BEGIN();

	RUN_TEST(test_a_broadcast_announce_is_accepted_without_a_reply);
	RUN_TEST(test_the_long_announce_word_is_also_an_announce);
	RUN_TEST(test_erase_and_run_words_are_recognised);
	RUN_TEST(test_a_unicast_to_us_may_be_answered);
	RUN_TEST(test_a_unicast_to_another_board_is_ignored_entirely);
	RUN_TEST(test_ordinary_application_traffic_draws_no_response);

	RUN_TEST(test_a_broadcast_with_our_serial_may_be_answered);
	RUN_TEST(test_a_broadcast_naming_another_board_is_ignored);
	RUN_TEST(test_a_board_with_no_identity_cannot_be_selected_by_serial);
	RUN_TEST(test_a_broadcast_with_our_uid_may_be_answered);
	RUN_TEST(test_an_unselected_broadcast_acts_but_does_not_answer);
	RUN_TEST(test_an_unselected_broadcast_adopt_is_refused);

	RUN_TEST(test_a_legacy_three_element_frame_is_accepted);
	RUN_TEST(test_a_good_trailer_is_verified_and_its_sequence_kept);
	RUN_TEST(test_a_corrupted_frame_is_rejected_rather_than_acted_on);
	RUN_TEST(test_no_corrupted_frame_is_ever_acted_on_with_altered_content);

	RUN_TEST(test_a_data_frame_carries_its_offset_and_payload);
	RUN_TEST(test_a_data_frame_with_a_bad_payload_checksum_is_rejected);
	RUN_TEST(test_a_large_offset_survives_the_full_bank);
	RUN_TEST(test_every_msgpack_offset_width_decodes_to_the_same_number);
	RUN_TEST(test_every_frame_offset_of_a_completely_full_bank_decodes);
	RUN_TEST(test_an_oversized_payload_is_refused_before_a_byte_is_read);
	RUN_TEST(test_a_payload_shorter_than_its_own_checksum_is_refused);

	RUN_TEST(test_a_truncated_frame_never_blocks);
	RUN_TEST(test_an_unknown_control_key_is_skipped_rather_than_fatal);
	RUN_TEST(test_an_unknown_verb_is_reported);
	RUN_TEST(test_frames_are_consumed_one_at_a_time_and_in_order);
	RUN_TEST(test_the_window_holds_one_frame_and_never_blocks);

	return UNITY_END();
}
