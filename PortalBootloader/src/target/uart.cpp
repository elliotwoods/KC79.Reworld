// GPIO and the two UARTs.
//
// USART2 is the RS485 bus: PA2/PA3 for data and PA1 as the transceiver's driver-enable, driven by
// the peripheral itself rather than by software. Hardware DE is what makes the turnaround exact --
// the line drops when the shift register empties, not when a loop gets round to noticing.
//
// USART1 is the debug UART on PB6/PB7, which is the ST-Link's virtual COM port. Transmit only, one
// character at a time, no formatting: the entire log vocabulary is single characters, because at
// 115200 on a board being driven by a host that is talking to fifty-three others, a sentence per
// event takes longer to send than the events take to happen.

#include "target.hpp"

namespace bl {
namespace target {

	int ringAvailable();
	int ringRead();
	int ringPeek();

	namespace {
		/// `USARTDIV` for 115200, computed at compile time for each clock this build can run at.
		///
		/// A constant rather than a division. `LL_USART_SetBaudRate` would divide at run time and
		/// pull in `__aeabi_uidiv` -- about 700 bytes of libgcc for an arithmetic result that is
		/// known here.
		constexpr uint32_t baud = 115200;
		constexpr uint32_t divisorFor(uint32_t sysclk) { return (sysclk + baud / 2u) / baud; }

		static_assert(divisorFor(64'000'000u) == 556, "64 MHz divisor");
		static_assert(divisorFor(16'000'000u) == 139, "16 MHz fallback divisor");

		void configurePin(GPIO_TypeDef * port, uint32_t pin, uint32_t mode, uint32_t alternate,
			uint32_t pull)
		{
			port->MODER = (port->MODER & ~(3u << (pin * 2u))) | (mode << (pin * 2u));
			port->PUPDR = (port->PUPDR & ~(3u << (pin * 2u))) | (pull << (pin * 2u));
			port->OSPEEDR &= ~(3u << (pin * 2u));
			port->OTYPER &= ~(1u << pin);
			if(pin < 8u) {
				port->AFR[0] = (port->AFR[0] & ~(0xFu << (pin * 4u))) | (alternate << (pin * 4u));
			}
			else {
				const uint32_t shift = (pin - 8u) * 4u;
				port->AFR[1] = (port->AFR[1] & ~(0xFu << shift)) | (alternate << shift);
			}
		}

		/// The RS485 side, as a stream the codec can read from and write to.
		class Rs485Stream : public msgpack::Stream {
		public:
			int available() override { return ringAvailable(); }
			int read() override { return ringRead(); }
			int peek() override { return ringPeek(); }

			size_t write(uint8_t value) override
			{
				while(!(USART2->ISR & USART_ISR_TXE_TXFNF)) {
				}
				USART2->TDR = value;
				return 1;
			}

			size_t write(const uint8_t * buffer, size_t size) override
			{
				for(size_t index = 0; index < size; index++) {
					this->write(buffer[index]);
				}
				return size;
			}

			void flush() override
			{
				// Transmission complete, not merely "the register is free". The driver-enable
				// line follows this flag, so returning early would drop the bus mid-byte.
				while(!(USART2->ISR & USART_ISR_TC)) {
				}
			}
		};

		Rs485Stream g_rs485;
	}

	//----------
	msgpack::Stream & rs485()
	{
		return g_rs485;
	}

	//----------
	void serialInit(uint32_t sysclk)
	{
		RCC->IOPENR |= RCC_IOPENR_GPIOAEN | RCC_IOPENR_GPIOBEN | RCC_IOPENR_GPIODEN;
		RCC->APBENR1 |= RCC_APBENR1_USART2EN;
		RCC->APBENR2 |= RCC_APBENR2_USART1EN;
		(void) RCC->APBENR2;

		// PA1 DE, PA2 TX, PA3 RX -- all USART2's alternate function 1.
		configurePin(GPIOA, 1, 2u, 1u, 0u);
		configurePin(GPIOA, 2, 2u, 1u, 0u);
		configurePin(GPIOA, 3, 2u, 1u, 0u);

		// PB6 TX, PB7 RX on USART1's alternate function 0.
		configurePin(GPIOB, 6, 2u, 0u, 0u);
		configurePin(GPIOB, 7, 2u, 0u, 0u);

		// PB3 and PB4 drive the two indicator LEDs.
		configurePin(GPIOB, 3, 1u, 0u, 0u);
		configurePin(GPIOB, 4, 1u, 0u, 0u);
		GPIOB->BSRR = (1u << (3u + 16u)) | (1u << (4u + 16u));

		// PD0-PD3 are the ID switches, active low, so they need pull-ups.
		for(uint32_t pin = 0; pin < 4u; pin++) {
			configurePin(GPIOD, pin, 0u, 0u, 1u);
		}

		const uint32_t divisor = (sysclk >= 32'000'000u)
			? divisorFor(64'000'000u)
			: divisorFor(16'000'000u);

		// USART2: 8N1, oversampling by 16, FIFO on, hardware driver-enable active high.
		USART2->CR1 = 0;
		USART2->BRR = divisor;
		USART2->CR2 = 0;
		USART2->CR3 = USART_CR3_DEM | USART_CR3_DEP;
		USART2->CR1 = USART_CR1_FIFOEN
			| USART_CR1_TE
			| USART_CR1_RE
			| USART_CR1_RXNEIE_RXFNEIE
			| USART_CR1_UE;

		// USART1: transmit only, same framing.
		USART1->CR1 = 0;
		USART1->BRR = divisor;
		USART1->CR2 = 0;
		USART1->CR3 = 0;
		USART1->CR1 = USART_CR1_FIFOEN | USART_CR1_TE | USART_CR1_UE;

		NVIC_SetPriority(USART2_IRQn, 0);
		NVIC_EnableIRQ(USART2_IRQn);
	}

} // namespace target
} // namespace bl
