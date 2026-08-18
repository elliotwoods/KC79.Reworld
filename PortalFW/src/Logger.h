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

// The USART1 console is a bare-keystroke menu: one byte in, one action, no terminator (append
// a newline and the firmware dispatches the newline as a second command). That is fine for
// "home now" but it cannot carry an argument, so nothing that needs a NUMBER -- set the
// threshold to 226, run a census at 240 -- was reachable over the console at all.
//
// LOGGER_LINE_PREFIX opens a second, additive door. A leading ':' starts a buffered line which
// runs when CR or LF arrives; anything not preceded by ':' is still a bare keystroke, byte for
// byte as before. Existing tools are unaffected and the two modes cannot be confused, because
// ':' was not previously bound to anything.
#define LOGGER_LINE_PREFIX ':'
#define LOGGER_LINE_MAX    48

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
#ifndef HOME_SWITCH_LEGACY
	void printHomeSwitchInfo();
#endif
	
	static std::shared_ptr<Logger> get();

	void log(const LogMessage&);
	void printRaw(const char *);

	void reportStatus(msgpack::Serializer&);

	std::vector<ILogListener*> logListeners;
private:
	Logger();

	// Run one ':' line command. See printHelp() for the vocabulary.
	void runLineCommand(char * line);

	std::deque<LogMessage> messageOutbox;
	std::map<char, MenuItem> menuItems;

	char lineBuffer[LOGGER_LINE_MAX];
	uint8_t lineLength = 0;
	bool lineMode = false;
};