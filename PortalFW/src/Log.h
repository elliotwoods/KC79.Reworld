#pragma once

#include "HardwareSerial.h"

#include <sstream>
#include <memory>
#include <vector>
#include <functional>

void initLoggerSerial();

enum LogLevel {
	Status
	, Warning
	, Error
};

struct LogMessage {
	LogLevel level;
	std::string message;
};

void log(const LogLevel&, const char* message);
void log(const LogMessage&);

class ILogListener {
public:
	virtual void onLogMessage(const LogMessage&) = 0;
};

class Logger {
public:
	static void setup();

	static Logger& X();
	static std::shared_ptr<Logger> get();

	void log(const LogMessage&);

	std::vector<ILogListener*> logListeners;
private:
	Logger();
};