#include "SerialStream.hpp"
#include "Logger.hpp"
#include <stdio.h>

void HAL_UARTEx_RxEventCallback(UART_HandleTypeDef *uart, uint16_t size)
{
	auto serialStream = SerialStream::getSerialStream(uart);
	serialStream->onRxCallback(size);
}

//----------
SerialStream::MappedStream SerialStream::mappedStreams[2];
SerialStream::MappedStream * SerialStream::mappedStreamsEnd = &SerialStream::mappedStreams[0];

//----------
SerialStream::SerialStream(UART_HandleTypeDef& uart, DMA_HandleTypeDef& dma)
: uart(uart)
, dma(dma)
{
	// Store mapped stream
	*mappedStreamsEnd++ = MappedStream {
		&uart
		, this
	};

	// Not inside an assert. `lwrb_init` is what gives this stream its ring buffer, and an
	// `assert` compiles to nothing when NDEBUG is defined -- which a release build does -- so the
	// call itself would disappear along with the check. The result is a SerialStream whose buffer
	// was never initialised, in the release build only, in the firmware that receives updates
	// over RS485. It happened not to bite because `ringBuffer` is a member of a
	// statically-allocated object and therefore starts zeroed, which is close enough to
	// initialised that the failure would have been intermittent rather than immediate.
	//
	// This is also why it did not compile here: CubeIDE's include list happened to pull `assert`
	// in transitively, and PlatformIO's does not. A missing declaration is a much better way to
	// find this than the alternative.
	const auto ringBufferReady = lwrb_init(&this->ringBuffer
		, this->ringBufferData
		, BUFFER_SIZE) == 1;
	if (!ringBufferReady) {
		// There is nowhere to report this to -- the object being constructed is what a log
		// message would travel over -- so the honest response is to stop rather than to run on
		// with a stream that silently drops everything.
		while (true) {
		}
	}
}

//----------
SerialStream::~SerialStream()
{
	lwrb_free(&this->ringBuffer);
}

//----------
void
SerialStream::init()
{
	this->activateDMARx();
}

//----------
size_t
SerialStream::write(uint8_t data)
{
	auto result = HAL_UART_Transmit(&this->uart
		  , &data
		  , 1
		  , 100);
	return result == HAL_StatusTypeDef::HAL_OK
			? 1
			: -1;
}

//----------
size_t
SerialStream::write(const char * data)
{
	auto length = strlen(data);
	return this->write((uint8_t*) data, length);
}

//----------
size_t
SerialStream::write(const uint8_t * data, size_t size)
{
	auto result = HAL_UART_Transmit(&this->uart
		  , data
		  , size
		  , 100);
	return result == HAL_StatusTypeDef::HAL_OK
			? size
			: -1;
}

//----------
void
SerialStream::flush()
{

}

//----------
int
SerialStream::available()
{
	if(__HAL_UART_GET_FLAG(&this->uart, UART_FLAG_ORE)) {
		log(LogLevel::Error, "Serial buffer overrun");

		// Clear the error flag
		__HAL_UART_CLEAR_FLAG(&this->uart, UART_CLEAR_OREF);

		// Restart the stream
		this->activateDMARx();
	}

	return lwrb_get_full(&this->ringBuffer);
}

//----------
int
SerialStream::read()
{
	// Check if buffer is empty
	{
		auto sizeInBuffer = lwrb_get_full(&this->ringBuffer);
		if(sizeInBuffer == 0) {
			return -1;
		}
	}

	// Read one byte off from buffer
	uint8_t data;
	lwrb_read(&this->ringBuffer
			, &data
			, 1);

	return (int) data;
}

//----------
int
SerialStream::peek()
{
	// Check if buffer is empty
	{
		auto sizeInBuffer = lwrb_get_full(&this->ringBuffer);
		if(sizeInBuffer == 0) {
			return -1;
		}
	}

	uint8_t data;
	lwrb_peek(&this->ringBuffer
			, 0
			, &data
			, 1);
	return (int) data;
}

//----------
int
SerialStream::readBytes(char* data, size_t length)
{
	// Check if buffer is empty
	{
		auto sizeInBuffer = lwrb_get_full(&this->ringBuffer);
		if(sizeInBuffer == 0) {
			return -1;
		}
	}

	return lwrb_read(&this->ringBuffer
			, data
			, length);
}

//----------
SerialStream *
SerialStream::getSerialStream(UART_HandleTypeDef* uart)
{
	for(auto mappedStream = &SerialStream::mappedStreams[0]; mappedStream != SerialStream::mappedStreamsEnd; mappedStream++) {
		if(mappedStream->uart == uart) {
			return mappedStream->serialStream;
		}
	}
	// Should never happen -- every UART this callback fires for was registered by a
	// SerialStream constructor -- but returning nullptr on the way out is a free
	// safety net against reading an uninitialized register as a pointer.
	return nullptr;
}

//----------
void
SerialStream::onRxCallback(uint16_t size)
{
	if(size == 0) {
		return;
	}

	auto bytesWritten = lwrb_write(&this->ringBuffer
					, this->device.data
					, size);

	if(bytesWritten != size) {
		log(LogLevel::Error, "RB Full");
	}

	// Re-activate the read request
	this->activateDMARx();
}

//----------
void
SerialStream::activateDMARx()
{
	HAL_UARTEx_ReceiveToIdle_DMA(&uart
			, this->device.data
			, BUFFER_SIZE);
	__HAL_DMA_DISABLE_IT(&this->dma, DMA_IT_HT);
}
