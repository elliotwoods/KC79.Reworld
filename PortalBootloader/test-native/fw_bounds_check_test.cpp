// Does the firmware-update frame parser reject an oversized or underflowing
// `bodyAndChecksumSize` before it would size a stack VLA from it?
//
// BootloaderRS485's original FWUpdateApp::processIncoming sized
// `uint8_t dataWithChecksum[packetBodyAndChecksumSize]` directly from a wire-supplied
// uint16, with no upper bound: a corrupt or malicious frame claiming up to 65,535 bytes
// would smash the stack of an image that can only be re-flashed via ST-Link. A value
// smaller than sizeof(CRCType) would also underflow `packetBodySize` (unsigned
// wraparound to a huge size_t). See protocol-hardening.md and the PortalBootloader plan
// for the fix: reject before the VLA is declared, same as the real
// PortalBootloader/cube-import/Core/Src/FWUpdateApp.cpp now does.
//
// This test replays the same msgpack decode sequence as fw_frame_offset_test.cpp (real
// COBSRWStream + deserialiser, non-Arduino path) and then applies the exact bounds-check
// condition the fixed FWUpdateApp.cpp uses, so a change to either constants.h or the
// condition itself is caught here rather than only discovered on the bench.
//
// Run: powershell -File run.ps1

#include <cstdint>
#include <cstdio>
#include <cstring>
#include <deque>
#include <vector>

#include <msgpack.hpp>

#include "../cube-import/Core/Src/constants.h"

namespace {

int failures = 0;
int checks = 0;

void check(bool ok, const char* what, uint32_t context)
{
	checks++;
	if (!ok) {
		failures++;
		std::printf("  FAIL  %s (context %u)\n", what, context);
	}
}

/// Same loopback stream as fw_frame_offset_test.cpp.
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

private:
	std::deque<uint8_t> data;
};

/// The bytes a host puts on the wire for one firmware frame, before COBS -- a fixmap(1)
/// keyed by frameOffset (fixint here, the offset isn't what's under test), whose bin
/// value declares `bodyAndChecksumSize` bytes but only actually carries `min(actualBytes,
/// bodyAndChecksumSize)` of them. This lets the test construct a frame that *claims* to
/// be huge without needing to actually transmit 65 KB.
std::vector<uint8_t> makeFrameBody(uint16_t declaredSize, const std::vector<uint8_t>& actualBytes)
{
	std::vector<uint8_t> body;
	body.push_back(0x81); // fixmap(1)
	body.push_back(0x00); // key: frameOffset = 0 (fixint)

	body.push_back(0xC5); // bin16
	body.push_back((uint8_t)(declaredSize >> 8));
	body.push_back((uint8_t)declaredSize);

	body.insert(body.end(), actualBytes.begin(), actualBytes.end());
	return body;
}

/// Mirrors FWUpdateApp::processIncoming's map-body branch up to (and including) the
/// bounds check, WITHOUT declaring the VLA -- so this test can safely probe sizes the
/// real firmware must reject, without actually needing a 65 KB stack.
///
/// Returns true if the frame is accepted by the bounds check (i.e. the real code would
/// go on to declare `dataWithChecksum[packetBodyAndChecksumSize]` and read into it).
bool wouldBeAccepted(uint16_t declaredSize, const std::vector<uint8_t>& actualBytes, bool& parseFailed)
{
	LoopbackStream loopback;
	msgpack::COBSRWStream cobs(loopback);

	const auto body = makeFrameBody(declaredSize, actualBytes);
	cobs.write(body.data(), body.size());
	cobs.flush();

	parseFailed = true;
	if (!msgpack::nextDataTypeIs(cobs, msgpack::DataType::Map)) {
		return false;
	}
	size_t mapSize = 0;
	if (!msgpack::readMapSize(cobs, mapSize, true) || mapSize != 1) {
		return false;
	}
	uint32_t frameOffset = 0;
	if (!msgpack::readInt(cobs, frameOffset, true)) {
		return false;
	}
	uint16_t packetBodyAndChecksumSize16 = 0;
	if (!msgpack::readBinarySize(cobs, packetBodyAndChecksumSize16, true)) {
		return false;
	}
	parseFailed = false;

	// The exact condition in PortalBootloader/cube-import/Core/Src/FWUpdateApp.cpp.
	typedef uint16_t CRCType;
	size_t checksumSize = sizeof(CRCType);
	if (packetBodyAndChecksumSize16 < checksumSize
		|| packetBodyAndChecksumSize16 > FW_FRAME_SIZE + FW_CHECKSUM_SIZE) {
		return false;
	}
	return true;
}

std::vector<uint8_t> makeBytes(size_t n)
{
	std::vector<uint8_t> out(n);
	for (size_t i = 0; i < n; i++) {
		out[i] = (uint8_t)(i * 31 + 7);
	}
	return out;
}

void testValidSizesAccepted()
{
	std::printf("legitimate frame sizes are accepted\n");

	// The Router's default 32-byte data chunk + 2-byte checksum = 34, and the boundary
	// case (a full FW_FRAME_SIZE chunk + checksum) must both still work.
	const uint16_t sizes[] = {2, 34, (uint16_t)(FW_FRAME_SIZE + FW_CHECKSUM_SIZE)};
	for (uint16_t size : sizes) {
		bool parseFailed = false;
		const bool accepted = wouldBeAccepted(size, makeBytes(size), parseFailed);
		check(!parseFailed, "frame parses", size);
		check(accepted, "declared size within bound is accepted", size);
	}
}

void testOversizedRejected()
{
	std::printf("oversized declared size is rejected before the VLA would be sized\n");

	// One byte over the boundary, and the pathological uint16-max case that used to risk
	// a 65 KB stack VLA. Real bytes are kept small -- only the *declared* size claims to
	// be huge, which is exactly the corrupt-frame scenario being guarded against.
	const uint16_t sizes[] = {(uint16_t)(FW_FRAME_SIZE + FW_CHECKSUM_SIZE + 1), 0xFFFF};
	for (uint16_t size : sizes) {
		bool parseFailed = false;
		const bool accepted = wouldBeAccepted(size, makeBytes(4), parseFailed);
		check(!parseFailed, "frame still parses (bin16 header itself is well-formed)", size);
		check(!accepted, "oversized declared size is rejected", size);
	}
}

void testUnderflowRejected()
{
	std::printf("a declared size smaller than the checksum is rejected, not underflowed\n");

	// size 0 and 1 are both < sizeof(CRCType) == 2; the original code's
	// `packetBodySize = packetBodyAndChecksumSize - checksumSize` would wrap to a huge
	// size_t for either.
	const uint16_t sizes[] = {0, 1};
	for (uint16_t size : sizes) {
		bool parseFailed = false;
		const bool accepted = wouldBeAccepted(size, makeBytes(size), parseFailed);
		check(!parseFailed, "frame still parses", size);
		check(!accepted, "underflowing declared size is rejected", size);
	}
}

} // namespace

int main()
{
	std::printf("FWUpdateApp bounds-check test (non-Arduino path, as the bootloader builds)\n");
	std::printf("FW_FRAME_SIZE=%d FW_CHECKSUM_SIZE=%d (max accepted declared size = %d)\n\n",
		FW_FRAME_SIZE, FW_CHECKSUM_SIZE, FW_FRAME_SIZE + FW_CHECKSUM_SIZE);

	testValidSizesAccepted();
	testOversizedRejected();
	testUnderflowRejected();

	std::printf("\n%d checks, %d failures\n", checks, failures);
	return failures == 0 ? 0 : 1;
}
