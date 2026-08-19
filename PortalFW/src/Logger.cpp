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
#ifndef HOME_SWITCH_LEGACY
		,{
			'd'
			, {
				"Home switch diagnostics (optical)"
				, [this]() {
					this->printHomeSwitchInfo();
				}
			}
		}
#endif
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
			const char c = (char) serial.read();

			// Buffered ':' line, terminated by CR or LF. Runs at most one line per update tick,
			// matching the keystroke path -- a routine started from here blocks inside
			// runLineCommand, and anything still in the RX buffer waits, which is what makes
			// "set the threshold then census" arrive as two commands rather than one race.
			if(this->lineMode) {
				if(c == '\r' || c == '\n') {
					this->lineBuffer[this->lineLength] = '\0';
					this->lineMode = false;
					const uint8_t length = this->lineLength;
					this->lineLength = 0;
					if(length > 0) {
						this->runLineCommand(this->lineBuffer);
					}
					return;
				}
				if(this->lineLength < LOGGER_LINE_MAX - 1) {
					this->lineBuffer[this->lineLength++] = c;
				}
				continue;
			}

			if(c == LOGGER_LINE_PREFIX) {
				this->lineMode = true;
				this->lineLength = 0;
				command = 0;
				continue;
			}

			// Keystrokes keep their original behaviour exactly: only the LAST byte of the tick
			// is acted on, so a paste or a held key cannot queue up a run of motions.
			command = c;
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
Logger::runLineCommand(char * line)
{
	// Split into a verb and up to three integer arguments. Deliberately tiny: this is a bench
	// console reached over a debug UART, not a protocol -- anything that needs structure goes
	// over RS485 as msgpack.
	char verb = line[0];
	long args[3] = { -1, -1, -1 };
	int argCount = 0;
	{
		char * cursor = line + 1;
		while(argCount < 3) {
			while(*cursor == ' ' || *cursor == ',' || *cursor == '\t') cursor++;
			if(*cursor == '\0') break;
			char * next = nullptr;
			const long value = strtol(cursor, &next, 10);
			if(next == cursor) break;
			args[argCount++] = value;
			cursor = next;
		}
	}

	switch(verb) {
#ifndef HOME_SWITCH_LEGACY
	case 't':
	{
		// The threshold DAC is shared by both axes (one PWM pin, one RC filter), so there is
		// deliberately no per-axis form of this.
		if(argCount >= 1) {
			long duty = args[0];
			if(duty < 0) duty = 0;
			if(duty > 255) duty = 255;
			Modules::HomeSwitchOptical::setThreshold((uint8_t) duty);
		}
		char message[64];
		sprintf(message, "threshold=%d\r\n", (int) Modules::HomeSwitchOptical::getThreshold());
		this->printRaw(message);
		break;
	}

	case 'n':
	{
		if(argCount < 1) {
			this->printRaw("usage: :n <threshold> [speed] [axis 0=A 1=B]\r\n");
			break;
		}
		long duty = args[0];
		if(duty < 0) duty = 0;
		if(duty > 255) duty = 255;
		const StepsPerSecond speed = (argCount >= 2 && args[1] > 0) ? (StepsPerSecond) args[1] : 0;
		const long axis = (argCount >= 3) ? args[2] : 0;

		Modules::MotionControl::MeasureRoutineSettings settings;
		auto * motionControl = (axis == 1)
			? Modules::App::X().motionControlB
			: Modules::App::X().motionControlA;
		auto exception = motionControl->homeSwitchCensusRoutine((uint8_t) duty, speed, settings);
		if(exception) {
			// Qualified: inside Logger, unqualified `log` finds Logger::log(const LogMessage&).
			::log(exception);
		}
		break;
	}

	case 'h':
	{
		const long axis = (argCount >= 1) ? args[0] : 0;
		long count = (argCount >= 2) ? args[1] : 1;
		if(axis < 0 || axis > 1 || count < 1 || count > 200) {
			this->printRaw("usage: :h <axis 0=A 1=B> [count 1..200]\r\n");
			break;
		}
		auto * motionControl = axis == 1
			? Modules::App::X().motionControlB
			: Modules::App::X().motionControlA;
		Modules::MotionControl::MeasureRoutineSettings settings;
		for(long run = 1; run <= count; run++) {
			char marker[80];
			sprintf(marker, "home campaign: axis=%c run=%ld/%ld\r\n"
				, axis == 1 ? 'B' : 'A', run, count);
			this->printRaw(marker);
			auto exception = motionControl->fastHomeRoutine(settings);
			if(exception) {
				::log(exception);
				break;
			}
			if(!Modules::App::X().persistOpticalCalibration(motionControl)) {
				::log(LogLevel::Error, motionControl->getName()
					, "home campaign stopped: calibration persistence failed");
				break;
			}
		}
		break;
	}

	case 'd':
		this->printHomeSwitchInfo();
		break;
#endif

	case 'v':
		this->printVersion();
		break;

	default:
		this->printRaw("line commands: :t [duty]  :n <duty> [speed] [axis]  :h <axis> [count]  :d  :v\r\n");
		break;
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

	// Both, because they answer different questions and can disagree. F_CPU is a compile-time
	// constant from the variant -- it says what the build expected. HAL_RCC_GetHCLKFreq() reads
	// the RCC registers, so it says what the PLL is actually doing right now. Only the second
	// one can confirm SystemClock_Config took effect, which is exactly what was in question
	// when the redundant mid-setup call was removed.
	serial.print("Clock Speed: ");
	serial.print(HAL_RCC_GetHCLKFreq() / 1000000);
	serial.print(" MHz actual, ");
	serial.print(F_CPU / 1000000);
	serial.println(" MHz expected");
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

#ifndef HOME_SWITCH_LEGACY
//----------
// What the optical front-end actually reports, right now, at this position.
//
// The intended use is interactive: jog with 0-9 to walk the prism round, pressing 'd' as you
// go, and watch where the sensor goes active and what crossing duty it reports there. That is
// how you find this unit's flag and its usable threshold band -- which cannot be taken from a
// previous board or a previous day, and on the production ring cannot be derived from the
// background at all, because the background never crosses at any threshold.
//
// The crossing probe settles the RC-filtered threshold DAC properly (~1.5 s per axis), so this
// is deliberately slow. It moves nothing.
void
Logger::printHomeSwitchInfo()
{
	auto & app = Modules::App::X();

	const auto thresholdBefore = Modules::HomeSwitchOptical::getThreshold();

	{
		char message[80];
		sprintf(message, "Home switch @ threshold %d (shared by both axes)"
			, (int) thresholdBefore);
		serial.println(message);
	}

	// The driver's FAULT line, which is wired and configured as an input but read nowhere else
	// in the firmware. It matters here because a sensor that never goes active looks exactly the
	// same whether the flag is invisible or the axis simply is not turning -- and the fault line
	// is the one signal that can tell those apart from software.
	for(uint8_t i = 0; i < 2; i++) {
		auto motorDriver = (i == 0) ? app.motorDriverA : app.motorDriverB;
		const auto & config = motorDriver->getConfig();
		char message[100];
		sprintf(message, "  motor %c: fault=%s enabled=%d"
			, config.AxisLabel
			, digitalRead(config.Fault) == LOW ? "YES (active low)" : "no"
			, (int) motorDriver->getEnabled());
		serial.println(message);
	}

	for(uint8_t i = 0; i < 2; i++) {
		auto motionControl = app.getMotionControl(i);

		// Live state first, at the threshold as it stands -- that is the bit homing actually
		// latches on.
		const bool active = motionControl->getHomeSwitchActive();

		bool railLo = false;
		const uint32_t timeoutTime = millis() + 20000;
		const int crossing = motionControl->probeHomeCrossing(railLo, timeoutTime);

		// Polarity, because it is easy to get backwards and the labels are the whole value of
		// this command: the crossing duty is INVERSE to reflectance -- it is a threshold the
		// signal has to get under, so a LOWER crossing means a MORE reflective surface, and the
		// sensor reads ACTIVE exactly when crossing < threshold. The home feature is a
		// reflector (a bright mark on a dark ring), so it has the lower crossing of the two.
		//
		// `railLo` is the reading at the BOTTOM of the probe's bracket, so when the probe found
		// no crossing at all:
		//   railLo == true  -> active even at the lowest threshold -> crossing below the
		//                      bracket -> brighter than measurable
		//   railLo == false -> inactive even at 255 -> crossing above 255 -> darker than
		//                      measurable, which is the NORMAL off-flag reading on the
		//                      injection-moulded ring, whose background never crosses anywhere.
		char message[190];
		if(crossing >= 0) {
			sprintf(message, "  %s: %s, crossing=%d, position=%d"
				, motionControl->getName()
				, active ? "ACTIVE" : "inactive"
				, crossing
				, (int) motionControl->getPosition());
		}
		else if(crossing == -1) {
			sprintf(message, "  %s: %s, crossing=censored (%s), position=%d"
				, motionControl->getName()
				, active ? "ACTIVE" : "inactive"
				, railLo
					? "always active below the bracket: brighter than measurable"
					: "never active up to 255: darker than measurable -- normal off-flag"
				, (int) motionControl->getPosition());
		}
		else {
			sprintf(message, "  %s: %s, crossing probe aborted/timed out, position=%d"
				, motionControl->getName()
				, active ? "ACTIVE" : "inactive"
				, (int) motionControl->getPosition());
		}
		serial.println(message);
	}

	// probeHomeCrossing leaves the DAC wherever its last probe put it, and the DAC is shared,
	// so restoring is not optional -- otherwise pressing 'd' silently re-thresholds both axes.
	Modules::HomeSwitchOptical::setThreshold(thresholdBefore);
}
#endif

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
