#pragma once

#include "stm32g0xx_hal.h"

enum LogLevel {
	Status
	, Warning
	, Error
};

struct LogMessage {
	LogLevel level;
	const char * message;
};

void setLoggerUART(UART_HandleTypeDef*);

void log(const LogLevel&, const char* message);
void log(const LogMessage&);
void logPrint(const char* message);
void logPrintBytes(const uint8_t* data, uint16_t size);
