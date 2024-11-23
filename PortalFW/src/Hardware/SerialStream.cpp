#include "./SerialStream.hpp"
#include <stdio.h>
#include <assert.h>

#include "Logger.h"

namespace Hardware {
	void HAL_UARTEx_RxEventCallback(UART_HandleTypeDef *uart, uint16_t size)
	{
		auto serialStream = SerialStream::getSerialStream(uart);
		serialStream->onRxCallback(size);
	}

	//----------
	SerialStream::MappedStream SerialStream::mappedStreams[2];
	SerialStream::MappedStream * SerialStream::mappedStreamsEnd = &SerialStream::mappedStreams[0];

	//----------
	SerialStream::SerialStream(USART_TypeDef * uart)
	{
		// This code comes from the BootloaderRS485's main.cpp

		// Init UART
		{
			this->uart.Instance = uart;
			this->uart.Init.BaudRate = 115200;
			this->uart.Init.WordLength = UART_WORDLENGTH_8B;
			this->uart.Init.StopBits = UART_STOPBITS_1;
			this->uart.Init.Parity = UART_PARITY_NONE;
			this->uart.Init.Mode = UART_MODE_TX_RX;
			this->uart.Init.HwFlowCtl = UART_HWCONTROL_NONE;
			this->uart.Init.OverSampling = UART_OVERSAMPLING_16;
			this->uart.Init.OneBitSampling = UART_ONE_BIT_SAMPLE_DISABLE;
			this->uart.Init.ClockPrescaler = UART_PRESCALER_DIV1;
			this->uart.AdvancedInit.AdvFeatureInit = UART_ADVFEATURE_NO_INIT;
		}

		// Store mapped stream (so we can find this stream later if needed from a static)
		*mappedStreamsEnd++ = MappedStream {
			&this->uart
			, this
		};

		assert(lwrb_init(&this->ringBuffer
			, this->ringBufferData
			, BUFFER_SIZE) == 1);
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
		// From BootloaderRS485. We refactored to move function calls to init
		
		// MX_DMA_Init
		{
			/* DMA controller clock enable */
			__HAL_RCC_DMA1_CLK_ENABLE();

			/* DMA interrupt init */
			/* DMA1_Channel1_IRQn interrupt configuration */
			HAL_NVIC_SetPriority(DMA1_Channel1_IRQn, 0, 0);
			HAL_NVIC_EnableIRQ(DMA1_Channel1_IRQn);
		}

		// MX_USART2_UART_Init
		{
			if (HAL_RS485Ex_Init(&this->uart, UART_DE_POLARITY_HIGH, 0, 0) != HAL_OK)
			{
				Error_Handler();
			}
			if (HAL_UARTEx_SetTxFifoThreshold(&this->uart, UART_TXFIFO_THRESHOLD_1_8) != HAL_OK)
			{
				Error_Handler();
			}
			if (HAL_UARTEx_SetRxFifoThreshold(&this->uart, UART_RXFIFO_THRESHOLD_1_8) != HAL_OK)
			{
				Error_Handler();
			}
			if (HAL_UARTEx_DisableFifoMode(&this->uart) != HAL_OK)
			{
				Error_Handler();
			}
		}

		// this->activateDMARx();

		this->write("Hello World\n");
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
			log(LogLevel::Error, "SerialStream", "Serial buffer overrun");

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
	size_t
	SerialStream::readBytes(char* data, size_t length)
	{
		// Check if buffer is empty
		{
			auto sizeInBuffer = lwrb_get_full(&this->ringBuffer);
			if(sizeInBuffer == 0) {
				return 0;
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
		// No error checking here - presume user knows what they're doing

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
			log(LogLevel::Error, "SerialStream", "RB Full");
		}

		// Re-activate the read request
		this->activateDMARx();
	}

	//----------
	void
	SerialStream::activateDMARx()
	{
		HAL_UARTEx_ReceiveToIdle_DMA(&this->uart
				, this->device.data
				, BUFFER_SIZE);
		__HAL_DMA_DISABLE_IT(&this->dma, DMA_IT_HT);
	}
}