// Running CRC-16/CCITT-FALSE over COBSRWStream, added for the application's RS485
// hardening (PortalFW/src/Modules/RS485.cpp's finishFrame()/checkChecksum() -- see the
// "streaming-friendly CRC-at-commit" design in the production-firmware plan). Lives here
// because this harness already builds the real submodule on the same non-Arduino path;
// the bootloader's own protocol is unaffected (it doesn't use the running CRC).
//
// getTxRunningCRC()/getRxRunningCRC() are always available (not Arduino-gated) specifically
// so this can be tested on the host. checkChecksum() itself is Arduino-only (it needs
// millis()) and isn't exercised here -- PortalFW/src/Modules/RS485.cpp is what calls it, on
// target.
//
// Run: powershell -File run.ps1

#include <cstdint>
#include <cstdio>
#include <cstring>
#include <deque>
#include <vector>

#include <msgpack.hpp>

namespace {

int failures = 0;
int checks = 0;

void check(bool ok, const char* what)
{
	checks++;
	if (!ok) {
		failures++;
		std::printf("  FAIL  %s\n", what);
	}
}

/// Same loopback stream shape as the other native tests here.
class LoopbackStream : public msgpack::Stream {
public:
	size_t write(uint8_t value) override
	{
		data.push_back(value);
		return 1;
	}
	size_t write(const uint8_t* buffer, size_t size) override
	{
		for (size_t i = 0; i < size; i++) data.push_back(buffer[i]);
		return size;
	}
	void flush() override {}
	int available() override { return (int)data.size(); }
	int read() override
	{
		if (data.empty()) return -1;
		const auto value = data.front();
		data.pop_front();
		return value;
	}
	int peek() override { return data.empty() ? -1 : data.front(); }

private:
	std::deque<uint8_t> data;
};

/// The known CRC-16/CCITT-FALSE test vector: CRC("123456789") == 0x29B1.
/// Confirms the TX-side running fold (folded in COBSRWStream::write, every logical byte,
/// before COBS zero-elimination) implements the algorithm protocol-hardening.md specifies,
/// independent of any COBS/framing behaviour.
void testKnownVector()
{
	std::printf("CRC-16/CCITT-FALSE known vector\n");

	LoopbackStream loopback;
	msgpack::COBSRWStream cobs(loopback);

	const uint8_t vector[] = "123456789";
	cobs.write(vector, 9);

	check(cobs.getTxRunningCRC() == 0x29B1, "CRC(\"123456789\") == 0x29B1");
}

/// writeEOP() resets the running CRC, so a second frame starts clean rather than carrying
/// the first frame's accumulator forward.
void testResetsBetweenFrames()
{
	std::printf("running CRC resets at writeEOP()\n");

	LoopbackStream loopback;
	msgpack::COBSRWStream cobs(loopback);

	cobs.write((const uint8_t*)"123456789", 9);
	check(cobs.getTxRunningCRC() == 0x29B1, "first frame reaches the known vector");
	cobs.writeEOP();
	check(cobs.getTxRunningCRC() == 0xFFFF, "reset to init after writeEOP()");

	cobs.write((const uint8_t*)"123456789", 9);
	check(cobs.getTxRunningCRC() == 0x29B1, "second frame reaches the same vector independently");
}

/// TX and RX must agree byte-for-byte on a real frame -- including one with embedded zero
/// bytes, which is the case COBS elimination exists for and the case that would expose a
/// fold happening on the wrong (zero-eliminated vs. logical) byte stream.
void testTxRxAgreeAcrossEmbeddedZeros()
{
	std::printf("TX and RX running CRCs agree across a frame with embedded zero bytes\n");

	LoopbackStream loopback;
	msgpack::COBSRWStream tx(loopback);

	// A representative envelope prefix with embedded zero bytes, matching the shape real
	// frames take: [target=-1, source=0, ...] via the forced-int8 header style
	// (RS485::makeHeader), which is exactly where the zero bytes live in a real broadcast.
	const uint8_t body[] = {0x93, 0xD0, 0xFF, 0xD0, 0x00, 0x81, 0xA1, 0x6D, 0x00, 0x2A};
	tx.write(body, sizeof(body));
	const uint16_t txCrc = tx.getTxRunningCRC();
	tx.writeEOP();

	msgpack::COBSRWStream rx(loopback);
	std::vector<uint8_t> decoded;
	// isStartOfIncomingPacket() means "before the first read of this packet" -- it goes
	// false as soon as the first byte is read, so it gates entry, not the loop itself.
	while (rx.available() > 0) {
		int b = rx.read();
		if (b < 0) break;
		decoded.push_back((uint8_t)b);
	}

	check(decoded.size() == sizeof(body), "RX decoded the same number of logical bytes");
	check(std::memcmp(decoded.data(), body, sizeof(body)) == 0, "RX decoded the same bytes");
	check(rx.getRxRunningCRC() == txCrc, "RX running CRC matches the TX snapshot taken at the same point");
}

} // namespace

int main()
{
	std::printf("COBSRWStream running-CRC test (non-Arduino path)\n\n");

	testKnownVector();
	testResetsBetweenFrames();
	testTxRxAgreeAcrossEmbeddedZeros();

	std::printf("\n%d checks, %d failures\n", checks, failures);
	return failures == 0 ? 0 : 1;
}
