#include "fake_hw.hpp"

#include "portal_crc32c.h"

#include <string.h>

namespace {
	constexpr uint32_t flashBytes = PORTAL_FLASH_END - PORTAL_FLASH_BASE;

	uint8_t g_flash[flashBytes];
	portal_handoff_t g_handoff;
	uint32_t g_uid[3] = {0x1122'3344u, 0x5566'7788u, 0x99AA'BBCCu};
	uint8_t g_dip = 0;
	uint32_t g_millis = 0;
	uint32_t g_kicks = 0;
	bool g_ringOverran = false;
	uint32_t g_replyGuards = 0;
	uint32_t g_erased = 0;
	uint32_t g_programmed = 0;
	uint32_t g_failErasePage = 0xFFFFFFFFu;
	uint32_t g_failProgramAt = 0xFFFFFFFFu;
	bltest::Terminal g_terminal;

	char g_log[8192];
	size_t g_logLength = 0;
	uint32_t g_ledToggles[2] = {0, 0};

	uint32_t offsetOf(uint32_t address) {
		return address - PORTAL_FLASH_BASE;
	}
}

namespace bltest {

	//----------
	void reset()
	{
		memset(g_flash, 0xFF, sizeof(g_flash));
		memset(&g_handoff, 0, sizeof(g_handoff));
		g_uid[0] = 0x1122'3344u;
		g_uid[1] = 0x5566'7788u;
		g_uid[2] = 0x99AA'BBCCu;
		g_dip = 0;
		g_millis = 0;
		g_kicks = 0;
		g_ringOverran = false;
		g_replyGuards = 0;
		g_erased = 0;
		g_programmed = 0;
		g_failErasePage = 0xFFFFFFFFu;
		g_failProgramAt = 0xFFFFFFFFu;
		g_terminal = Terminal();
		g_logLength = 0;
		g_log[0] = '\0';
		g_ledToggles[0] = 0;
		g_ledToggles[1] = 0;
	}

	uint8_t * flash() { return g_flash; }
	uint8_t * flashAt(uint32_t address) { return g_flash + offsetOf(address); }

	void eraseRegion(uint32_t address, uint32_t length)
	{
		memset(g_flash + offsetOf(address), 0xFF, length);
	}

	void preload(uint32_t address, const uint8_t * data, uint32_t length)
	{
		memcpy(g_flash + offsetOf(address), data, length);
	}

	void preloadApplication(uint32_t base, uint32_t entryOffset, bool descriptor,
		uint32_t descriptorBase, const char * version)
	{
		uint8_t * at = flashAt(base);
		const uint32_t stackPointer = PORTAL_RAM_END;
		// `| 1` rather than `+ 1`: the Thumb bit is a bit, and an entry offset that happened to be
		// odd would have it cleared by an addition.
		const uint32_t resetVector = (base + entryOffset) | 1u;
		memcpy(at, &stackPointer, 4);
		memcpy(at + 4, &resetVector, 4);

		if(descriptor) {
			portal_app_descriptor_t block;
			memset(&block, 0, sizeof(block));
			memcpy(block.magic, PORTAL_APP_DESCRIPTOR_MAGIC, 8);
			block.app_base = descriptorBase;
			block.flags = 0;
			if(version != nullptr) {
				size_t length = strlen(version);
				if(length > sizeof(block.version)) {
					length = sizeof(block.version);
				}
				memcpy(block.version, version, length);
			}
			memcpy(at + PORTAL_APP_DESCRIPTOR_OFFSET, &block, sizeof(block));
		}
	}

	uint32_t erasedPages() { return g_erased; }
	uint32_t programmedWords() { return g_programmed; }
	void failEraseOf(uint32_t page) { g_failErasePage = page; }
	void failProgramAt(uint32_t address) { g_failProgramAt = address; }

	void setUid(uint32_t a, uint32_t b, uint32_t c) { g_uid[0] = a; g_uid[1] = b; g_uid[2] = c; }
	void setDip(uint8_t value) { g_dip = value; }

	void setMillis(uint32_t value) { g_millis = value; }
	void advance(uint32_t by) { g_millis += by; }
	uint32_t watchdogKicks() { return g_kicks; }
	void setRingOverran() { g_ringOverran = true; }
	uint32_t replyGuards() { return g_replyGuards; }

	const Terminal & terminal() { return g_terminal; }

	const char * log() { return g_log; }
	void clearLog() { g_logLength = 0; g_log[0] = '\0'; }
	uint32_t ledToggles(bl::hw::Led led) { return g_ledToggles[(uint8_t) led]; }

} // namespace bltest

// ---- The hardware seam, implemented against the fakes above -------------------------------------

namespace bl {
namespace hw {

	//----------
	uint32_t flashErasePage(uint32_t page)
	{
		if(page == g_failErasePage) {
			g_failErasePage = 0xFFFFFFFFu;
			return 0x1u;
		}
		const uint32_t address = PORTAL_FLASH_BASE + page * PORTAL_FLASH_PAGE_BYTES;
		memset(g_flash + offsetOf(address), 0xFF, PORTAL_FLASH_PAGE_BYTES);
		g_erased++;
		return 0;
	}

	//----------
	uint32_t flashProgram8(uint32_t address, const uint8_t * data)
	{
		if(address == g_failProgramAt) {
			g_failProgramAt = 0xFFFFFFFFu;
			return 0x8u;
		}

		uint8_t * target = g_flash + offsetOf(address);
		// The rule that matters: this part will not program a double-word that is not erased.
		// PROGERR, and the write does not take.
		for(uint32_t index = 0; index < 8; index++) {
			if(target[index] != 0xFF) {
				return 0x8u;
			}
		}
		memcpy(target, data, 8);
		g_programmed++;
		return 0;
	}

	//----------
	const uint8_t * flashPtr(uint32_t address)
	{
		return g_flash + offsetOf(address);
	}

	//----------
	void uid(uint32_t out[3])
	{
		out[0] = g_uid[0];
		out[1] = g_uid[1];
		out[2] = g_uid[2];
	}

	//----------
	uint8_t dip()
	{
		return g_dip;
	}

	//----------
	portal_handoff_t * handoff()
	{
		return &g_handoff;
	}

	//----------
	uint32_t millis()
	{
		return g_millis;
	}

	//----------
	void replyGuard()
	{
		g_replyGuards++;
	}

	//----------
	bool ringOverran()
	{
		const bool value = g_ringOverran;
		g_ringOverran = false;
		g_replyGuards = 0;
		return value;
	}

	//----------
	void watchdogKick()
	{
		g_kicks++;
	}

	//----------
	void ledToggle(Led led)
	{
		g_ledToggles[(uint8_t) led]++;
	}

	//----------
	void ledSet(Led, bool)
	{
	}

	//----------
	void logChar(char value)
	{
		if(g_logLength + 1 < sizeof(g_log)) {
			g_log[g_logLength++] = value;
			g_log[g_logLength] = '\0';
		}
	}

	//----------
	void logString(const char * text)
	{
		while(*text != '\0') {
			logChar(*text++);
		}
	}

	//----------
	void txDrain()
	{
		g_terminal.drains++;
	}

	//----------
	void reset()
	{
		g_terminal.reset = true;
	}

	//----------
	void runApplication(uint32_t base)
	{
		g_terminal.ran = true;
		g_terminal.base = base;
	}

	//----------
	bool terminalActionsHalt()
	{
		return false;
	}

} // namespace hw
} // namespace bl
