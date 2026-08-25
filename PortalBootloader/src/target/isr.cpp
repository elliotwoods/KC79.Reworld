// The interrupt handlers, the receive ring, and the flash routines -- all resident in SRAM.
//
// # Why these live in RAM
//
// Erasing or programming flash on this part stalls every flash *read* until the operation
// finishes: 20-25 ms for a page erase. Code fetched from flash cannot execute during that, which
// includes the UART receive interrupt. The v4/v5 bootloader erased 52 pages in one blocking call
// and was therefore deaf for well over a second, with its DMA ring overflowing the whole time --
// which is why the host had to blanket each erase in three seconds of announce frames, send the
// erase twice, and still could not tell whether a board had heard it.
//
// With the handlers, the vector table and the flash routines all in SRAM, reception continues
// through an erase. That is what makes `begin` able to answer when it is actually finished, rather
// than the host guessing.
//
// Everything here is therefore written to touch nothing outside SRAM: no `memcpy`, no 64-bit
// arithmetic that could call into libgcc, no LL inline that might not inline. `tools/size_gate.py`
// checks the built image for exactly that, because a single accidental call back into flash would
// reintroduce the deafness silently.

#include "target.hpp"

#define RAM_FUNCTION __attribute__((section(".RamFunc"), noinline))

namespace {
	/// Raw bytes from USART2, before COBS. A power of two so the wrap is a mask.
	///
	/// 2 kB holds roughly 175 ms of continuous traffic at 115200, which is more than a page erase
	/// needs and enough that a host streaming 128-byte chunks 2 ms apart cannot outrun a main loop
	/// that is busy programming.
	constexpr uint32_t ringBytes = 2048;
	constexpr uint32_t ringMask = ringBytes - 1;

	volatile uint8_t g_ring[ringBytes];
	volatile uint16_t g_head = 0;
	volatile uint16_t g_tail = 0;
	volatile uint8_t g_overrun = 0;

	volatile uint32_t g_ticks = 0;
}

extern "C" {

	//----------
	RAM_FUNCTION void USART2_IRQHandler(void)
	{
		// Drain the FIFO in one visit rather than re-entering per byte.
		while(USART2->ISR & USART_ISR_RXNE_RXFNE) {
			const uint8_t value = (uint8_t) USART2->RDR;
			const uint16_t next = (uint16_t) ((g_head + 1u) & ringMask);
			if(next == g_tail) {
				// Full. Dropping the oldest would splice two frames together; dropping everything
				// costs one frame and resynchronises at the next delimiter.
				g_overrun = 1;
				g_head = g_tail;
			}
			else {
				g_ring[g_head] = value;
				g_head = next;
			}
		}

		// ORE raises this interrupt while RXNEIE is set, and it is not cleared by reading RDR. An
		// unhandled overrun therefore re-enters this handler forever, which on a bus that has just
		// glitched is a hang rather than a dropped byte.
		USART2->ICR = USART_ICR_ORECF | USART_ICR_FECF | USART_ICR_NECF | USART_ICR_PECF;
	}

	//----------
	void SysTick_Handler(void)
	{
		g_ticks++;
	}

} // extern "C"

namespace bl {
namespace target {

	//----------
	uint32_t ticks()
	{
		return g_ticks;
	}

	//----------
	int ringAvailable()
	{
		return (int) ((g_head - g_tail) & ringMask);
	}

	//----------
	int ringRead()
	{
		if(g_head == g_tail) {
			return -1;
		}
		const uint8_t value = g_ring[g_tail];
		g_tail = (uint16_t) ((g_tail + 1u) & ringMask);
		return (int) value;
	}

	//----------
	int ringPeek()
	{
		if(g_head == g_tail) {
			return -1;
		}
		return (int) g_ring[g_tail];
	}

	//----------
	bool ringOverran()
	{
		const bool value = g_overrun != 0;
		g_overrun = 0;
		return value;
	}

} // namespace target
} // namespace bl

// ---- Flash, also from RAM ---------------------------------------------------------------------

namespace {
	constexpr uint32_t flashKey1 = 0x45670123u;
	constexpr uint32_t flashKey2 = 0xCDEF89ABu;
	/// Every `rc_w1` status bit, so one write clears the lot.
	constexpr uint32_t statusClear = 0x0000C3FBu;
	/// The error bits within it.
	constexpr uint32_t statusErrors = 0x0000C3FAu;

	RAM_FUNCTION void flashUnlock()
	{
		if(FLASH->CR & FLASH_CR_LOCK) {
			FLASH->KEYR = flashKey1;
			FLASH->KEYR = flashKey2;
		}
	}

	RAM_FUNCTION void flashLock()
	{
		FLASH->CR |= FLASH_CR_LOCK;
	}
}

namespace bl {
namespace hw {

	//----------
	RAM_FUNCTION uint32_t flashErasePage(uint32_t page)
	{
		while(FLASH->SR & FLASH_SR_BSY1) {
		}
		flashUnlock();
		if(FLASH->CR & FLASH_CR_LOCK) {
			return 1u;
		}

		FLASH->SR = statusClear;
		FLASH->CR = (FLASH->CR & ~FLASH_CR_PNB_Msk)
			| (page << FLASH_CR_PNB_Pos)
			| FLASH_CR_PER
			| FLASH_CR_STRT;

		// The watchdog is reloaded from inside the wait, not around it: a page erase is a
		// meaningful fraction of the ~4.1 s period, and 53 of them back to back are not.
		while(FLASH->SR & FLASH_SR_BSY1) {
			IWDG->KR = 0xAAAAu;
		}

		FLASH->CR &= ~(FLASH_CR_PER | FLASH_CR_PNB_Msk);
		const uint32_t error = FLASH->SR & statusErrors;
		FLASH->SR = statusClear;
		while(FLASH->SR & FLASH_SR_CFGBSY) {
		}
		flashLock();
		return error;
	}

	//----------
	RAM_FUNCTION uint32_t flashProgram8(uint32_t address, const uint8_t * data)
	{
		// Assembled a byte at a time into two 32-bit halves. A `uint64_t` store here would emit a
		// call to a libgcc helper that lives in flash, which is exactly what this section exists
		// to avoid -- and the source may be unaligned, which a wider load would fault on.
		const uint32_t low = (uint32_t) data[0]
			| ((uint32_t) data[1] << 8)
			| ((uint32_t) data[2] << 16)
			| ((uint32_t) data[3] << 24);
		const uint32_t high = (uint32_t) data[4]
			| ((uint32_t) data[5] << 8)
			| ((uint32_t) data[6] << 16)
			| ((uint32_t) data[7] << 24);

		while(FLASH->SR & FLASH_SR_BSY1) {
		}
		flashUnlock();
		if(FLASH->CR & FLASH_CR_LOCK) {
			return 1u;
		}

		FLASH->SR = statusClear;
		FLASH->CR |= FLASH_CR_PG;

		*(volatile uint32_t *) address = low;
		__ISB();
		*(volatile uint32_t *) (address + 4u) = high;

		while(FLASH->SR & FLASH_SR_BSY1) {
		}

		FLASH->CR &= ~FLASH_CR_PG;
		const uint32_t error = FLASH->SR & statusErrors;
		FLASH->SR = statusClear;
		while(FLASH->SR & FLASH_SR_CFGBSY) {
		}
		flashLock();
		return error;
	}

} // namespace hw
} // namespace bl
