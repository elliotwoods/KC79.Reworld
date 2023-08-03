#pragma once

#include "HardwareSerial.h"

#include <sstream>
#include <memory>
#include <deque>

#define LOGGER_HISTORY_SIZE 10

void initLoggerSerial();

enum LogLevel {
	Status
	, Warning
	, Error
};
void log(const LogLevel&, const char* format, ...);
void log(const char* format, ...);

class Logger {
public:
	static void setup();

	static Logger& X();
	static std::shared_ptr<Logger> get();

	void print(const char *);
	void print(const char * format, ...);
	void print(const std::string &);

	const std::string& getLastMessage() const;
private:
	Logger();
	std::deque<std::string> messageHistory;
};