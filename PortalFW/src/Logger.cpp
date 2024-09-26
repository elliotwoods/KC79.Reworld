#include "Logger.h"
#include <Arduino.h>
#include <assert.h>
#include <msgpack.hpp>
#include "Modules/App.h"
#include "Version.h"

#pragma mark Log

#define LOG_MESSAGE_LENGTH 64

HardwareSerial serial(PB7, PB6);

//----------
void
log(const LogLevel& level, const char* module, const char* message, bool sendToServer)
{
	log((LogMessage) {
		level
		, module
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

//----------
void
log(const Exception& exception)
{
	log((LogMessage) {
		LogLevel::Error
		, exception.getModule()
		, exception.getMessage()
		, true
		, millis()
		});
}

//----------
void
print(const LogMessage& logMessage)
{
	// Special cases for begin and end messages
	if(logMessage.message == "begin" && logMessage.level == LogLevel::Status) {
		serial.println("/---------");
		serial.print("| BEGIN ");
		serial.println(logMessage.module.c_str());
		return;
	}
	else if(logMessage.message == "end" && logMessage.level == LogLevel::Status) {
		serial.print("| END ");
		serial.println(logMessage.module.c_str());
		serial.println("\\---------");
		serial.println("");
		return;
	}

	// Header section
	{
		serial.print("[");
		switch(logMessage.level) {
		case LogLevel::Status:
			break;
		case LogLevel::Warning:
			serial.print("W ");
			break;
		case LogLevel::Error:
			serial.print("E ");
			break;
		default:
			break;
		}

		serial.print(logMessage.module.c_str());

		serial.print("] ");
	}

	serial.print(logMessage.message.c_str());
	serial.println("");
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
	serial.println();
	this->printVersion();
	this->printHelp();
}

//----------
void
Logger::update()
{
	// take commands (one per update loop)
	{
		char command = 0;
		while(serial.available()) {
			command = serial.read();
		}
		if(command == 0) {
			// Do nothing
		}
		else if(command >= '0' && command <= '9') {
			auto stepsPerRevolution = Modules::App::X().motionControlA->getMicrostepsPerPrismRotation();
			auto targetPosition = stepsPerRevolution * (Steps) (command - '0') / (Steps) ('9' - '0');

			// Move to test position
			Modules::App::X().motionControlA->setTargetPosition(targetPosition);
			Modules::App::X().motionControlB->setTargetPosition(targetPosition);
		}
		else {
			switch(command) {
			case 'c':
				// Calibrate
				Modules::App::X().routines->calibrate();
				break;
			case 'h':
				// Home routine
				Modules::App::X().routines->home();
				break;
			case 's':
				// Startup
				Modules::App::X().routines->startup();
				break;
			case 'u':
				// Unjam
				Modules::App::X().routines->unjam();
				break;
			case 'y':
				// Measure cycle
				Modules::App::X().routines->measureCycle();
				break;
			case 'r':
				// Reboot
				NVIC_SystemReset();
				break;
			case 'v':
				// Version
				this->printVersion();
				break;
			case 27:
				// Escape from routine
				Modules::App::X().escapeFromRoutine();
				break;
			case '?':
				// Help
				this->printHelp();
				break;
			default:
				this->printOutbox();
			}
		}
	}
}

//----------
void
Logger::printOutbox()
{
	serial.println("---------------");
	serial.println("MESSAGE OUTBOX:");
	serial.println("---------------");
	serial.println("--");

	auto & messageOutbox = this->messageOutbox;

	// Cycle through the current message outbox
	for(auto logMessage : messageOutbox) {
		// We make a local copy of each log message

		// Don't log these messages to server (because they're already in outbox)
		logMessage.sendToServer = false;

		// put it through the logger again
		this->log(logMessage);
	}

	serial.println("--");
	serial.println("---------------");
	serial.println();
}

//----------
void
Logger::printVersion()
{
	serial.println(PORTAL_VERSION_STRING);
}

//----------
void
Logger::printHelp()
{
	serial.println("c = calibrate");
	serial.println("h = home routine");
	serial.println("s = startup");
	serial.println("u = unjam");
	serial.println("y = measure cycle");
	serial.println("v = print version");
	serial.println("0-9 = move to test position");
	serial.println("? = print help");
	serial.println("r = reboot");
	serial.println("ESC = exit current routine");
	serial.println("any other key = print the message outbox");
}

//----------
void
Logger::log(const LogMessage& logMessage)
{
	// Print to serial
	::print(logMessage);

	// Notify all log listeners
	for(auto logListener : this->logListeners) {
		logListener->onLogMessage(logMessage);
	}

	// Add it to the outbox to the server
	if(logMessage.sendToServer) {
		this->messageOutbox.push_back(logMessage);
		while(this->messageOutbox.size() > LOG_HISTORY_SIZE) {
			this->messageOutbox.pop_front();
		}
	}
}

//----------
void
Logger::printRaw(const char * message)
{
	serial.print(message);
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