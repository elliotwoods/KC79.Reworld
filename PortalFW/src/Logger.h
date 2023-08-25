#pragma once

#include "HardwareSerial.h"

#include <sstream>
#include <memory>
#include <vector>
#include <deque>
#include <functional>
#include <msgpack.hpp>

#define LOG_HISTORY_SIZE 32

void initLoggerSerial();

enum LogLevel : uint8_t {
	Status = 0
	, Warning = 10
	, Error = 20
};

struct LogMessage {
	LogLevel level;
	std::string message;
	bool sendToServer;
	uint32_t timestamp_ms;
};

void log(const LogLevel&, const char* message, bool sendToServer = true);
void log(const LogMessage&);

class ILogListener {
public:
	virtual void onLogMessage(const LogMessage&) = 0;
};

class Logger {
public:
	static Logger& X();

	void setup();
	void update();

	void printVersion();
	void printHelp();
	void printOutbox();
	
	static std::shared_ptr<Logger> get();

	void log(const LogMessage&);

	void reportStatus(msgpack::Serializer&);

	std::vector<ILogListener*> logListeners;
private:
	Logger();

	std::deque<LogMessage> messageOutbox;
};