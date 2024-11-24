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
	this->menuItems = {
		{
			'a'
			, {
				"Print axes info"
				, [this]() {
					this->printAxesInfo();
				}
			}
		}
		, {
			'c'
			, {
				"Calibrate"
				, []() {
					Modules::App::X().routines->calibrate();
				}
			}
		}
		,{
			'h'
			, {
				"Home routine"
				, []() {
					Modules::App::X().routines->calibrate();
				}
			}
		}
		,{
			's'
			, {
				"Startup routine"
				, []() {
					Modules::App::X().routines->startup();
				}
			}
		}
		,{
			'u'
			, {
				"Unjam routine"
				, []() {
					Modules::App::X().routines->unjam();
				}
			}
		}
		,{
			'y'
			, {
				"Measure cycle routine"
				, []() {
					Modules::App::X().routines->measureCycle();
				}
			}
		}
		,{
			'r'
			, {
				"Reboot"
				, []() {
					NVIC_SystemReset();
				}
			}
		}
		,{
			'v'
			, {
				"Print version"
				, [this]() {
					this->printVersion();
				}
			}
		}
		,{
			27
			, {
				"Escape routine"
				, [this]() {
					Modules::App::X().escapeFromRoutine();
				}
			}
		}
		,{
			'?'
			, {
				"Help"
				, [this]() {
					this->printHelp();
				}
			}
		}
	};
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
			// Check through commands
			auto it = this->menuItems.find(command);
			if(it != this->menuItems.end()) {
				// Perform action
				it->second.action();
			}
			else {
				// Default action
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

	// Print the clock speed
	serial.print("Clock Speed: ");
	serial.print(F_CPU / 1000000);
	serial.println(" MHz");
}

//----------
void
Logger::printHelp()
{
	for(auto & menuItem : this->menuItems) {
		// print the key
		switch(menuItem.first) {
		case 27:
			serial.print("ESC");
			break;
		case ' ':
			serial.print("Space");
		default:
			serial.print(menuItem.first);
		}

		serial.print(" = ");

		serial.println(menuItem.second.name.c_str());
	}
	serial.println("0-9 = Move to test position");
	serial.println("Any other key = Print the message outbox");
}

void
sprintf_fixed(char * string, float number, int dp)
{
	auto isNegative = number < 0;
	auto remaining = abs(number);

	auto wholePart = int(remaining);
	sprintf(string, "%s%d.", (isNegative ? "-" : " "), wholePart);
	remaining -= wholePart;

	for(int i=0; i<dp; i++) {
		remaining *= 10;
		wholePart = int(remaining);
		sprintf(string, "%s%d", string, int(remaining));
		remaining -= wholePart;
	}
}

//----------
void
Logger::printAxesInfo()
{
	float microstepsPerRotation = Modules::App::X().motionControlA->getMicrostepsPerPrismRotation();

	char axesInfo[2][100];

	for(uint8_t i=0; i<2; i++) {
		// Get the data
		auto motionControl = Modules::App::X().getMotionControl(i);
		auto moduleName = motionControl->getName();

		// Create the message
		{
			auto currentPosition = (float) motionControl->getPosition() / (float) microstepsPerRotation;
			auto targetPosition = (float) motionControl->getTargetPosition() / (float) microstepsPerRotation;

			char currentPosition_s[100];
			sprintf_fixed(currentPosition_s, currentPosition, 3);

			char targetPosition_s[100];
			sprintf_fixed(targetPosition_s, targetPosition, 3);

			sprintf(axesInfo[i], "%s\t->\t[%s]", currentPosition_s, targetPosition_s);
		}
	}

	char message[200];
	sprintf(message, "{A : %s, B: \t%s}", axesInfo[0], axesInfo[1]);
	::log(LogLevel::Status, "Logger::printAxisInfo", message);

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