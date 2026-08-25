// Does a firmware-update frame whose offset is past 65535 survive the wire?
//
// The Portal's application bank is 0x08006000..0x0801E800 = 100,352 bytes; the final three
// pages are durable identity/settings storage. The last application frame starts at offset
// 100,320. There has been a long-standing worry that something in
// this path narrows the offset to 16 bits and thereby caps the uploadable image at 64 kB.
//
// This settles it on a PC, against the *same* COBSRWStream and deserialiser the RS485 bootloader
// links -- and on the same non-Arduino code path, because the bootloader is a bare HAL project,
// not an Arduino one. It replays the exact parse sequence of BootloaderRS485's
// FWUpdateApp::processIncoming:
//
//     nextDataTypeIs(Map) -> readMapSize -> readInt<uint32_t> -> readBinarySize -> readRaw
//
// Body layout, matching Router's FWUpdate::uploadFirmwarePacket and RouterRS's
// fw_frame_envelope: fixmap(1) { <offset:uint> : bin(<checksum:u16 LE> ++ <data>) }.
//
// Note this exercises the library's current 256-byte decode buffer. The bootloader image fielded
// today carries a snapshot of this library with a 64-byte buffer, in which a 32-byte frame at a
// uint32 offset occupies 47 of those 64 bytes -- it fits, but with no margin. That is an argument
// for unifying on the submodule, not a reason to distrust the offset. Build with
// -DMSGPACK_COBSRWSTREAM_BUFFER_SIZE_OVERRIDE to check the tight case if the header ever gains a
// guard (today its #define is unconditional, so a -D would simply lose).
//
// Run: powershell -File run.ps1

#include <cstdint>
#include <cstdio>
#include <cstring>
#include <deque>
#include <vector>

#include <msgpack.hpp>

namespace {

constexpr uint32_t APP_FLASH_ADDRESS = 0x08006000;
constexpr uint32_t FLASH_END = 0x0801E800;
constexpr uint32_t APP_BANK_BYTES = FLASH_END - APP_FLASH_ADDRESS;
constexpr size_t FRAME_DATA_SIZE = 32;

int failures = 0;
int checks = 0;

void check(bool ok, const char* what, uint32_t context)
{
	checks++;
	if (!ok) {
		failures++;
		std::printf("  FAIL  %s (offset %u)\n", what, context);
	}
}

/// A stream that hands back whatever was written to it, so one COBSRWStream both encodes and
/// decodes. Deliberately on the msgpack::Stream (non-Arduino) path -- that is what the
/// bootloader compiles against.
class LoopbackStream : public msgpack::Stream {
public:
	size_t write(uint8_t value) override
	{
		data.push_back(value);
		return 1;
	}

	size_t write(const uint8_t* buffer, size_t size) override
	{
		for (size_t i = 0; i < size; i++) {
			data.push_back(buffer[i]);
		}
		return size;
	}

	void flush() override {}

	int available() override { return (int)data.size(); }

	int read() override
	{
		if (data.empty()) {
			return -1;
		}
		const auto value = data.front();
		data.pop_front();
		return value;
	}

	int peek() override { return data.empty() ? -1 : data.front(); }

	size_t encodedSize() const { return data.size(); }

private:
	std::deque<uint8_t> data;
};

/// BootloaderRS485 Core/Src/FWUpdateApp.cpp calcCheckSum, and Router's Utils::calcCheckSum:
/// XOR of 16-bit little-endian words.
uint16_t calcCheckSum(const uint8_t* data, size_t size)
{
	uint16_t value = 0;
	for (size_t i = 0; i + 1 < size; i += 2) {
		value ^= (uint16_t)(data[i] | ((uint16_t)data[i + 1] << 8));
	}
	return value;
}

/// The bytes a host puts on the wire for one firmware frame, before COBS. The key uses the
/// minimal unsigned width, as both msgpack_pack_uint32 (C++ host) and dump_uint (Rust host) do.
std::vector<uint8_t> makeFrameBody(uint32_t frameOffset, const std::vector<uint8_t>& data)
{
	std::vector<uint8_t> body;
	body.push_back(0x81); // fixmap(1)

	if (frameOffset < 128) {
		body.push_back((uint8_t)frameOffset);
	}
	else if (frameOffset <= 0xFF) {
		body.push_back(0xCC);
		body.push_back((uint8_t)frameOffset);
	}
	else if (frameOffset <= 0xFFFF) {
		body.push_back(0xCD);
		body.push_back((uint8_t)(frameOffset >> 8));
		body.push_back((uint8_t)frameOffset);
	}
	else {
		body.push_back(0xCE);
		body.push_back((uint8_t)(frameOffset >> 24));
		body.push_back((uint8_t)(frameOffset >> 16));
		body.push_back((uint8_t)(frameOffset >> 8));
		body.push_back((uint8_t)frameOffset);
	}

	const size_t withChecksum = data.size() + sizeof(uint16_t);
	body.push_back(0xC4); // bin8
	body.push_back((uint8_t)withChecksum);

	const uint16_t checksum = calcCheckSum(data.data(), data.size());
	body.push_back((uint8_t)(checksum & 0xFF));
	body.push_back((uint8_t)(checksum >> 8));

	body.insert(body.end(), data.begin(), data.end());
	return body;
}

std::vector<uint8_t> makeData(uint32_t seed)
{
	std::vector<uint8_t> data(FRAME_DATA_SIZE);
	for (size_t i = 0; i < data.size(); i++) {
		data[i] = (uint8_t)(seed + i * 31 + 7);
	}
	return data;
}

struct Parsed {
	bool parsed = false;
	uint32_t offset = 0;
	bool checksumOk = false;
	std::vector<uint8_t> data;
	size_t encodedBytes = 0;
};

/// Round-trip one frame through COBS and parse it exactly as the bootloader does.
Parsed roundTripFrame(uint32_t frameOffset, const std::vector<uint8_t>& data)
{
	Parsed out;

	LoopbackStream loopback;
	msgpack::COBSRWStream cobs(loopback);

	const auto body = makeFrameBody(frameOffset, data);
	cobs.write(body.data(), body.size());
	cobs.flush();
	out.encodedBytes = loopback.encodedSize();

	if (!msgpack::nextDataTypeIs(cobs, msgpack::DataType::Map)) {
		return out;
	}

	size_t mapSize = 0;
	if (!msgpack::readMapSize(cobs, mapSize, true) || mapSize != 1) {
		return out;
	}

	// The line the whole 16-bit worry is about. uint32_t in, uint32_t out.
	if (!msgpack::readInt(cobs, out.offset, true)) {
		return out;
	}

	uint16_t bodyAndChecksumSize = 0;
	if (!msgpack::readBinarySize(cobs, bodyAndChecksumSize, true)) {
		return out;
	}

	std::vector<uint8_t> withChecksum(bodyAndChecksumSize);
	if (!msgpack::readRaw(cobs, (char*)withChecksum.data(), bodyAndChecksumSize, true)) {
		return out;
	}

	constexpr size_t checksumSize = sizeof(uint16_t);
	const uint16_t transmitted = (uint16_t)(withChecksum[0] | ((uint16_t)withChecksum[1] << 8));
	const uint16_t calculated =
		calcCheckSum(withChecksum.data() + checksumSize, bodyAndChecksumSize - checksumSize);

	out.checksumOk = (transmitted == calculated);
	out.data.assign(withChecksum.begin() + checksumSize, withChecksum.end());
	out.parsed = true;
	return out;
}

void expectRoundTrip(uint32_t frameOffset)
{
	const auto data = makeData(frameOffset);
	const auto got = roundTripFrame(frameOffset, data);

	check(got.parsed, "frame parsed", frameOffset);
	if (!got.parsed) {
		return;
	}
	check(got.offset == frameOffset, "offset survived", frameOffset);
	check(got.checksumOk, "checksum matched", frameOffset);
	check(got.data == data, "payload survived", frameOffset);
}

// ---------------------------------------------------------------- the cases

/// The case that matters: the final frame of a completely full application bank.
void testLastFrameOfFullApplicationBank()
{
	std::printf("last frame of a full application bank\n");

	const uint32_t offset = APP_BANK_BYTES - FRAME_DATA_SIZE; // 100,320
	expectRoundTrip(offset);

	// Pin the exact wire bytes of the key, so a change of encoding is visible here rather than
	// as a mystery on the bench.
	const auto body = makeFrameBody(offset, makeData(offset));
	check(body[0] == 0x81, "fixmap(1)", offset);
	check(body[1] == 0xCE, "uint32 key marker, not uint16", offset);
	check(body[2] == 0x00 && body[3] == 0x01 && body[4] == 0x87 && body[5] == 0xE0,
		"key bytes 00 01 87 E0", offset);
	check(body[6] == 0xC4 && body[7] == 34, "bin8 of 34", offset);
}

/// Either side of every encoding-width boundary, so a regression is localised rather than just
/// "big images fail".
void testOffsetsAcrossEncodingWidths()
{
	std::printf("offsets across every msgpack width boundary\n");

	const uint32_t offsets[] = {
		0,     // positive fixint
		127,   // last fixint
		128,   // uint8
		255,   // last uint8
		256,   // uint16
		65535, // last uint16
		65536, // uint32 - the boundary the worry is about
		100000,
	};
	for (uint32_t offset : offsets) {
		expectRoundTrip(offset);
	}
}

/// Every frame start of a full image, so nothing wraps or aliases anywhere in the bank.
void testEveryFrameOffsetInAFullImage()
{
	std::printf("every frame offset in the bounded 100,352-byte application bank\n");

	uint32_t frames = 0;
	for (uint32_t offset = 0; offset + FRAME_DATA_SIZE <= APP_BANK_BYTES;
		offset += (uint32_t)FRAME_DATA_SIZE) {
		const auto data = makeData(offset);
		const auto got = roundTripFrame(offset, data);
		if (!got.parsed || got.offset != offset || !got.checksumOk || got.data != data) {
			failures++;
			std::printf("  FAIL  offset %u round-tripped as %u (parsed=%d, checksum=%d)\n",
				offset, got.offset, (int)got.parsed, (int)got.checksumOk);
			break;
		}
		frames++;
	}
	checks++;
	std::printf("  %u frames\n", frames);
}

/// Proves the assertions above are live rather than vacuous.
///
/// Deliberately encode the last frame's offset as a uint16, which is what a 16-bit narrowing
/// anywhere in the host would produce, and confirm the parser faithfully reports the truncated
/// value. 100,320 & 0xFFFF = 34,784 -- an address 65,536 bytes short, landing back inside the
/// image and silently corrupting it. If this test ever reports 100,320, the round-trip checks
/// above are not actually reading the key and none of them mean anything.
void testANarrowedKeyWouldBeCaught()
{
	std::printf("a deliberately 16-bit-narrowed key is visibly wrong\n");

	const uint32_t trueOffset = APP_BANK_BYTES - FRAME_DATA_SIZE; // 100,320
	const uint32_t truncated = trueOffset & 0xFFFF;               // 34,784

	const auto data = makeData(trueOffset);
	const auto got = roundTripFrame(truncated, data);

	check(got.parsed, "narrowed frame still parses", truncated);
	check(got.offset == truncated, "parser reports the truncated offset", truncated);
	check(got.offset != trueOffset, "truncation is detectable", truncated);
	check(truncated == 34784, "truncation arithmetic", truncated);
}

/// The write address the bootloader computes must stay inside the bank for every frame.
void testWriteAddressStaysInBank()
{
	std::printf("computed write addresses stay inside the bank\n");

	const uint32_t lastOffset = APP_BANK_BYTES - FRAME_DATA_SIZE;
	const uint32_t lastAddress = APP_FLASH_ADDRESS + lastOffset;

	check(lastAddress == 0x0801E7E0, "last write address is 0x0801E7E0", lastOffset);
	check(lastAddress + FRAME_DATA_SIZE == FLASH_END, "last frame ends exactly at flash end",
		lastOffset);
}

} // namespace

int main()
{
	std::printf("msgpack firmware-frame offset test (non-Arduino path, as the bootloader builds)\n");
	std::printf("COBS decode buffer: %d bytes\n\n", (int)MSGPACK_COBSRWSTREAM_BUFFER_SIZE);

	testLastFrameOfFullApplicationBank();
	testOffsetsAcrossEncodingWidths();
	testEveryFrameOffsetInAFullImage();
	testANarrowedKeyWouldBeCaught();
	testWriteAddressStaysInBank();

	std::printf("\n%d checks, %d failures\n", checks, failures);
	return failures == 0 ? 0 : 1;
}
