#include "Logger.hpp"
#include <string.h>

UART_HandleTypeDef * loggerUART;

//----------
void
setLoggerUART(UART_HandleTypeDef* uart)
{
	loggerUART = uart;
}

//----------
void
log(const LogLevel& logLevel, const char * message)
{
	log(LogMessage {
		logLevel
		, message
	});
}

//----------
void
log(const LogMessage& logMessage)
{
	logPrint(logMessage.message);
}

//----------
void
logPrint(const char * text)
{
	logPrintBytes((uint8_t*) text
	  , strlen(text));
}

//----------
void logPrintBytes(const uint8_t* data, uint16_t size)
{
	HAL_UART_Transmit(loggerUART
		  , data
		  , size
		  , 100);
}
