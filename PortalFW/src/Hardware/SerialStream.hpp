#pragma once

#include "stm32g0xx_hal.h"
#include <map>

#include "msgpack/lwrb.h"
#include "Stream.h"

#define BUFFER_SIZE 256

namespace Hardware {
	class SerialStream : public Stream {
	public:
		SerialStream(USART_TypeDef * uart);
		SerialStream(UART_HandleTypeDef&, DMA_HandleTypeDef&);
		~SerialStream();

		void init() ;
		size_t write(uint8_t) override;
		size_t write(const char *);
		size_t write(const uint8_t * buffer, size_t size) override;
		void flush() override;

		int available() override;
		int read() override;
		int peek() override;
		size_t readBytes(char * data, size_t length) override;

		static SerialStream * getSerialStream(UART_HandleTypeDef *);
		void onRxCallback(uint16_t size);
	protected:
		void activateDMARx();
		UART_HandleTypeDef uart;
		DMA_HandleTypeDef dma;

		struct MappedStream {
			UART_HandleTypeDef* uart;
			SerialStream* serialStream;
		};
		static MappedStream mappedStreams[2];
		static MappedStream * mappedStreamsEnd;

		struct {
			uint8_t data[BUFFER_SIZE];
		} device;

		lwrb_t ringBuffer;
		uint8_t ringBufferData[BUFFER_SIZE];
	};
}