#include "Log.h"
#include <Arduino.h>

#pragma mark Log

#define LOG_MESSAGE_LENGTH 64

HardwareSerial serial(PB7, PB6);

//----------
void
log(const LogLevel& level, const char* message)
{
	log((LogMessage) {
		level
		, message
		});
}

//----------
void
log(const LogMessage& message)
{
	Logger::X().log(message);
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
Logger::log(const LogMessage& logMessage)
{
	switch(logMessage.level) {
	case LogLevel::Status:
		break;
	case LogLevel::Warning:
		serial.print("[W] ");
		break;
	case LogLevel::Error:
		serial.print("[E] ");
		break;
	default:
		break;
	}

	serial.println(logMessage.message.c_str());
	for(auto logListener : this->logListeners) {
		logListener->onLogMessage(logMessage);
	}
}