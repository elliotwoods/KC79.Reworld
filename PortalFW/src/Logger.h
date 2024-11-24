#pragma once

#include "HardwareSerial.h"
#include "Exception.h"

#include <sstream>
#include <memory>
#include <vector>
#include <map>
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
	std::string module;
	std::string message;
	bool sendToServer;
	uint32_t timestamp_ms;
};

void log(const LogLevel&, const char* module, const char* message, bool sendToServer = true);
void log(const LogMessage&);
void log(const Exception&);

void print(const LogMessage&);

class ILogListener {
public:
	virtual void onLogMessage(const LogMessage&) = 0;
};

class Logger {
public:
	struct MenuItem {
		std::string name;
		std::function<void()> action;
	};

	static Logger& X();

	void setup();
	void update();

	void printVersion();
	void printHelp();
	void printOutbox();
	void printAxesInfo();
	
	static std::shared_ptr<Logger> get();

	void log(const LogMessage&);
	void printRaw(const char *);

	void reportStatus(msgpack::Serializer&);

	std::vector<ILogListener*> logListeners;
private:
	Logger();

	std::deque<LogMessage> messageOutbox;
	std::map<char, MenuItem> menuItems;
};