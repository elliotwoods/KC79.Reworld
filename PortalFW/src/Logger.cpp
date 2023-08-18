#include "Logger.h"
#include <Arduino.h>
#include <assert.h>
#include <msgpack.hpp>

#pragma mark Log

#define LOG_MESSAGE_LENGTH 64

HardwareSerial serial(PB7, PB6);

//----------
void
log(const LogLevel& level, const char* message, bool sendToServer)
{
	log((LogMessage) {
		level
		, message
		, sendToServer
		, millis()
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
	assert(lwrb_init(&this->messageRingBuffer
		, this->messageRingBufferData
		, LOG_HISTORY_SIZE) == 1);
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

	if(logMessage.sendToServer) {
		this->messageOutbox.push_back(logMessage);
		while(this->messageOutbox.size() > LOG_HISTORY_SIZE) {
			this->messageOutbox.pop_front();
		}
	}
}

//----------
void
Logger::reportStatus(msgpack::Serializer& serializer)
{
	auto count = this->messageOutbox.size();

	serializer.beginArray(count);
	for(size_t i=0; i<count; i++) {
		auto & message = this->messageOutbox.front();
		serializer.beginMap(3);
		{
			serializer << "level" << (uint8_t) message.level;
			serializer << "message" << message.message.c_str();
			serializer << "timestamp" << message.timestamp_ms;
		}
		this->messageOutbox.pop_front();
	}
}