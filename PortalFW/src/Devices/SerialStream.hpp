#pragma once

#include "msgpack.hpp"
#include "stm32g0xx_hal.h"
#include <map>

#include "lwrb.h"

#define BUFFER_SIZE 256

class SerialStream : public Stream {
public:
	SerialStream(UART_HandleTypeDef&, DMA_HandleTypeDef&);
	~SerialStream();

	void init();
	size_t write(uint8_t);
	size_t write(const char *);
	size_t write(const uint8_t * buffer, size_t size);
	void flush();
	int available();
	int read();
	int peek();
	size_t readBytes(char * data, size_t length);

	static SerialStream * getSerialStream(UART_HandleTypeDef *);
	void onRxCallback(uint16_t size);
protected:
	void activateDMARx();
	UART_HandleTypeDef & uart;
	DMA_HandleTypeDef & dma;

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
