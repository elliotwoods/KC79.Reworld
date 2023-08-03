#include "Log.h"
#include <Arduino.h>

#pragma mark Log

#define LOG_MESSAGE_LENGTH 64

HardwareSerial serial(PB7, PB6);

//----------
void
log(const LogLevel& level, const char* format, ...)
{
	va_list args;
	va_start(args, format);
	Logger::X().print(format, args);
	va_end(args);
}

//----------
void
log(const char* format, ...)
{
	va_list args;
	va_start(args, format);
	Logger::X().print(format, args);
	va_end(args);
}

#pragma mark Logger

//---------
Logger&
Logger::X()
{
	return * Logger::get();
}

//---------
std::shared_ptr<Logger>
Logger::get()
{
	static std::shared_ptr<Logger> instance = std::shared_ptr<Logger>(new Logger());
	return instance;
}

//---------
Logger::Logger()
{

}

//----------
void
Logger::setup()
{
	serial.begin(115200);
	serial.println("APP START");
}

//----------
void
Logger::print(const char * message)
{
	serial.print(message);
	this->messageHistory.push_back(std::string(message));

	while(this->messageHistory.size() > LOGGER_HISTORY_SIZE) {
		this->messageHistory.pop_front();
	}
}

//----------
void
Logger::print(const char * format, ...)
{
	char message[LOG_MESSAGE_LENGTH];
	{
		va_list args;
		va_start(args, format);
		sprintf(message, format, args);
		va_end(args);
	}

	serial.println(message);
	this->messageHistory.push_back(std::string(message));

	while(this->messageHistory.size() > LOGGER_HISTORY_SIZE) {
		this->messageHistory.pop_front();
	}
}

//----------
void
Logger::print(const std::string & message)
{
	serial.println(message.c_str());
	this->messageHistory.push_back(message);

	while(this->messageHistory.size() > LOGGER_HISTORY_SIZE) {
		this->messageHistory.pop_front();
	}
}

//----------
const std::string&
Logger::getLastMessage() const
{
	if(this->messageHistory.empty()) {
		static std::string emptyString;
		return emptyString;
	}
	else {
		return this->messageHistory.back();
	}
}