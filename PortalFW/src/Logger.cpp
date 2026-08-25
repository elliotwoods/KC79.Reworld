#include "Logger.h"
#include <Arduino.h>
#include <assert.h>
#include <msgpack.hpp>
#include "Modules/App.h"
#include "Version.h"

#pragma mark Log

#define LOG_MESSAGE_LENGTH 64

HardwareSerial serial(PB7, PB6);
msgpack::COBSRWStream directStream(serial);

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
	if(Logger::X().directActive()) {
		Logger::X().sendDirectLog(logMessage);
		return;
	}
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
				"Cycle check (both axes)"
				, []() {
					Modules::App::X().routines->cycleCheck();
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
	if(this->directMode) {
		this->updateDirect();
		return;
	}
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
	case 'b':
	{
		if(argCount != 2 || args[0] != 1 || args[1] < 0) {
			this->printRaw("usage: :b <protocol=1> <nonce>\r\n");
			break;
		}
		serial.print("DIRECT 1 ");
		serial.println((unsigned long) args[1]);
		serial.flush();
		this->directMode = true;
		this->directHeartbeatMs = millis();
		this->lineMode = false;
		this->lineLength = 0;
		break;
	}
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
		serial.print("Provision Serial: ");
		serial.println((unsigned long) Modules::App::X().getProvisionSerial());
		this->printVersion();
		break;

	default:
		this->printRaw("line commands: :b <version> <nonce>  :t [duty]  :n <duty> [speed] [axis]  :h <axis> [count]  :d  :v\r\n");
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
	if(this->directMode) {
		return;
	}
	serial.print(message);
}

namespace {
	enum DirectKind : uint8_t {
		DIRECT_HELLO = 1, DIRECT_HEARTBEAT = 2, DIRECT_EXIT = 3,
		DIRECT_STATUS = 4, DIRECT_OP = 5, DIRECT_JOG = 6,
		DIRECT_SURVEY_START = 7, DIRECT_ABORT = 8,
		DIRECT_ACK = 64, DIRECT_ERROR = 65, DIRECT_STATUS_EVENT = 66,
		DIRECT_LOG_EVENT = 67, DIRECT_SURVEY_BEGIN = 68,
		DIRECT_SURVEY_SAMPLE = 69, DIRECT_SURVEY_END = 70
	};

	void directFrameBegin(uint8_t seq, uint8_t kind)
	{
		msgpack::writeArraySize4(directStream, 5);
		msgpack::writeIntU7(directStream, 1);
		msgpack::writeIntU7(directStream, seq);
		msgpack::writeIntU7(directStream, kind);
	}

	void directFrameEnd()
	{
		msgpack::writeIntU16(directStream, directStream.getTxRunningCRC());
		directStream.flush();
	}
}

//----------
void
Logger::sendDirectAck(uint8_t seq)
{
	directFrameBegin(seq, DIRECT_ACK);
	msgpack::writeNil(directStream);
	directFrameEnd();
}

//----------
void
Logger::sendDirectError(uint8_t seq, const char * detail)
{
	directFrameBegin(seq, DIRECT_ERROR);
	msgpack::writeArraySize4(directStream, 2);
	msgpack::writeIntU7(directStream, 1);
	msgpack::writeString(directStream, detail);
	directFrameEnd();
}

//----------
void
Logger::sendDirectStatus(uint8_t seq)
{
	auto & app = Modules::App::X();
	directFrameBegin(seq, DIRECT_STATUS_EVENT);
	msgpack::writeArraySize4(directStream, 12);
	msgpack::writeInt32(directStream, app.motionControlA->getPosition());
	msgpack::writeInt32(directStream, app.motionControlB->getPosition());
	msgpack::writeInt32(directStream, app.motionControlA->getTargetPosition());
	msgpack::writeInt32(directStream, app.motionControlB->getTargetPosition());
	const auto & a = app.motionControlA->getHealthStatus();
	const auto & b = app.motionControlB->getHealthStatus();
	msgpack::writeBool(directStream, a.measureCycleOK);
	msgpack::writeBool(directStream, a.switchesOK);
	msgpack::writeBool(directStream, a.backlashOK);
	msgpack::writeBool(directStream, a.homeOK);
	msgpack::writeBool(directStream, b.measureCycleOK);
	msgpack::writeBool(directStream, b.switchesOK);
	msgpack::writeBool(directStream, b.backlashOK);
	msgpack::writeBool(directStream, b.homeOK);
	directFrameEnd();
}

//----------
void
Logger::sendDirectLog(const LogMessage& message)
{
	directFrameBegin(this->directTxSeq++, DIRECT_LOG_EVENT);
	msgpack::writeArraySize4(directStream, 3);
	msgpack::writeIntU8(directStream, (uint8_t) message.level);
	std::string text = message.module + ": " + message.message;
	msgpack::writeString(directStream, text.c_str());
	msgpack::writeIntU32(directStream, message.timestamp_ms);
	directFrameEnd();
}

//----------
void
Logger::updateDirect()
{
	if(millis() - this->directHeartbeatMs > 3000U) {
		Modules::App::X().motionControlA->stop();
		Modules::App::X().motionControlB->stop();
		Modules::App::X().escapeFromRoutine();
		this->directMode = false;
		this->lineMode = false;
		return;
	}

	if(!directStream.isStartOfIncomingPacket()) {
		directStream.nextIncomingPacket();
	}
	while(directStream.isStartOfIncomingPacket() && directStream.available()) {
		this->processDirectPacket();
		if(directStream.isEndOfIncomingPacket()) {
			directStream.nextIncomingPacket();
		}
		else {
			break;
		}
	}
}

//----------
bool
Logger::processDirectPacket()
{
	size_t frameSize = 0;
	uint8_t version = 0, seq = 0, kind = 0;
	if(!msgpack::readArraySize(directStream, frameSize)
		|| frameSize != 5
		|| !msgpack::readInt<uint8_t>(directStream, version)
		|| !msgpack::readInt<uint8_t>(directStream, seq)
		|| !msgpack::readInt<uint8_t>(directStream, kind)) {
		this->sendDirectError(seq, "invalid direct frame");
		return false;
	}

	uint8_t axisIndex = 0, surveyMode = 0, dutyMin = 0, dutyMax = 0;
	int32_t speed = 0, center = 0, halfRange = 0, step = 0, value = 0;
	uint8_t opCode = 0;
	bool bodyOK = true;

	switch(kind) {
	case DIRECT_HEARTBEAT:
	case DIRECT_EXIT:
	case DIRECT_STATUS:
	case DIRECT_ABORT:
		bodyOK = msgpack::readNil(directStream);
		break;
	case DIRECT_JOG:
	{
		size_t count = 0;
		bodyOK = msgpack::readArraySize(directStream, count) && count == 2
			&& msgpack::readInt<uint8_t>(directStream, axisIndex)
			&& msgpack::readInt<int32_t>(directStream, speed);
		break;
	}
	case DIRECT_SURVEY_START:
	{
		size_t count = 0;
		bool centerIsHome = false;
		bodyOK = msgpack::readArraySize(directStream, count) && count == 8
			&& msgpack::readInt<uint8_t>(directStream, axisIndex)
			&& msgpack::readInt<uint8_t>(directStream, surveyMode)
			&& msgpack::readInt<int32_t>(directStream, center)
			&& msgpack::readBool(directStream, centerIsHome)
			&& msgpack::readInt<int32_t>(directStream, halfRange)
			&& msgpack::readInt<int32_t>(directStream, step)
			&& msgpack::readInt<uint8_t>(directStream, dutyMin)
			&& msgpack::readInt<uint8_t>(directStream, dutyMax);
		break;
	}
	case DIRECT_OP:
	{
		size_t count = 0;
		bodyOK = msgpack::readArraySize(directStream, count) && count >= 2
			&& msgpack::readInt<uint8_t>(directStream, opCode);
		if(bodyOK && opCode == 3) {
			bodyOK = count == 2 && msgpack::readInt<int32_t>(directStream, value);
		}
		else {
			bodyOK = bodyOK && msgpack::readInt<uint8_t>(directStream, axisIndex);
			if(bodyOK && opCode == 2) {
				bodyOK = count == 3 && msgpack::readInt<int32_t>(directStream, value);
			}
		}
		break;
	}
	default:
		bodyOK = false;
		break;
	}

	const uint16_t calculated = directStream.getRxRunningCRC();
	uint16_t transmitted = 0;
	bodyOK = bodyOK && msgpack::readIntU16(directStream, transmitted, true);
	if(!bodyOK || version != 1 || transmitted != calculated) {
		this->sendDirectError(seq, "direct CRC or body invalid");
		return false;
	}
	this->directHeartbeatMs = millis();

	auto * axis = axisIndex == 1
		? Modules::App::X().motionControlB
		: Modules::App::X().motionControlA;

	switch(kind) {
	case DIRECT_HEARTBEAT:
		this->sendDirectStatus(seq);
		return true;
	case DIRECT_EXIT:
		Modules::App::X().motionControlA->stop();
		Modules::App::X().motionControlB->stop();
		this->sendDirectAck(seq);
		this->directMode = false;
		return true;
	case DIRECT_STATUS:
		this->sendDirectStatus(seq);
		return true;
	case DIRECT_ABORT:
		Modules::App::X().motionControlA->stop();
		Modules::App::X().motionControlB->stop();
		Modules::App::X().escapeFromRoutine();
		this->sendDirectAck(seq);
		return true;
	case DIRECT_JOG:
		if(axisIndex > 1 || speed < -14080 || speed > 14080) {
			this->sendDirectError(seq, "jog outside safe range");
			return false;
		}
		if(speed == 0) axis->stop();
		else axis->run(speed > 0, speed > 0 ? speed : -speed);
		this->sendDirectAck(seq);
		return true;
#ifndef HOME_SWITCH_LEGACY
	case DIRECT_SURVEY_START:
		if(axisIndex > 1 || surveyMode > 1 || halfRange <= 0 || step <= 0
			|| halfRange > 20000 || dutyMin > dutyMax
			|| ((uint32_t) halfRange * 2U / (uint32_t) step) + 1U > 4096U) {
			this->sendDirectError(seq, "survey bounds invalid");
			return false;
		}
		this->runDirectSurvey(seq, axis, surveyMode, center, halfRange, step
			, dutyMin, dutyMax);
		return true;
#endif
	case DIRECT_OP:
		if(axisIndex > 1 && opCode != 3) {
			this->sendDirectError(seq, "axis invalid");
			return false;
		}
		if(opCode == 1) {
#ifndef HOME_SWITCH_LEGACY
			Modules::MotionControl::MeasureRoutineSettings settings;
			auto error = axis->fastHomeRoutine(settings);
			if(error) this->sendDirectError(seq, error.getMessage().c_str());
			else this->sendDirectAck(seq);
#else
			this->sendDirectError(seq, "optical home unavailable");
#endif
		}
		else if(opCode == 2) {
			auto result = axis->routineMoveTo(value, millis() + 30000U);
			if(result.exception) this->sendDirectError(seq, result.exception.getMessage().c_str());
			else this->sendDirectAck(seq);
		}
		else if(opCode == 3) {
#ifndef HOME_SWITCH_LEGACY
			if(value < 0) value = 0;
			if(value > 255) value = 255;
			Modules::HomeSwitchOptical::setThreshold((uint8_t) value);
			this->sendDirectAck(seq);
#else
			this->sendDirectError(seq, "optical threshold unavailable");
#endif
		}
		else this->sendDirectError(seq, "direct op unsupported");
		return true;
	default:
		this->sendDirectError(seq, "direct kind unsupported");
		return false;
	}
}

#ifndef HOME_SWITCH_LEGACY
//----------
void
Logger::runDirectSurvey(uint8_t seq, Modules::MotionControl * axis
	, uint8_t mode, int32_t center, int32_t halfRange, int32_t step
	, uint8_t dutyMin, uint8_t dutyMax)
{
	const uint32_t expected = ((uint32_t) halfRange * 2U / (uint32_t) step) + 1U;
	const uint8_t thresholdBefore = Modules::HomeSwitchOptical::getThreshold();
	directFrameBegin(seq, DIRECT_SURVEY_BEGIN);
	msgpack::writeArraySize4(directStream, 1);
	msgpack::writeIntU32(directStream, expected);
	directFrameEnd();

	bool aborted = false;
	const int32_t first = center - halfRange;
	for(uint32_t index = 0; index < expected; index++) {
		const int32_t position = first + (int32_t) index * step;
		auto move = axis->routineMoveTo(position, millis() + 30000U);
		if(move.exception || Modules::App::getShouldEscapeFromRoutine()
			|| !this->directMode) {
			aborted = true;
			break;
		}

		int crossing = -1;
		bool railLo = false;
		uint8_t sampleClass = 0;
		if(mode == 1) {
			crossing = axis->probeHomeCrossing(railLo, millis() + 20000U);
			if(crossing == -1) sampleClass = railLo ? 1 : 2;
			else if(crossing < 0) sampleClass = 3;
			else if(crossing < dutyMin) { crossing = -1; sampleClass = 1; }
			else if(crossing > dutyMax) { crossing = -1; sampleClass = 2; }
		}
		else {
			bool activeAtMin = false;
			for(uint16_t duty = dutyMin; duty <= dutyMax; duty++) {
				Modules::HomeSwitchOptical::setThreshold((uint8_t) duty);
				HAL_Delay(20);
				const bool active = axis->getHomeSwitchActive();
				if(duty == dutyMin) activeAtMin = active;
				if(active && !activeAtMin) { crossing = duty; break; }
			}
			if(activeAtMin) sampleClass = 1;
			else if(crossing < 0) sampleClass = 2;
		}

		directFrameBegin(this->directTxSeq++, DIRECT_SURVEY_SAMPLE);
		msgpack::writeArraySize4(directStream, 5);
		msgpack::writeIntU32(directStream, index);
		msgpack::writeInt32(directStream, position);
		msgpack::writeInt32(directStream, position - center);
		if(crossing >= 0) msgpack::writeIntU8(directStream, (uint8_t) crossing);
		else msgpack::writeNil(directStream);
		msgpack::writeIntU7(directStream, sampleClass);
		directFrameEnd();
	}

	axis->routineMoveTo(center, millis() + 30000U);
	Modules::HomeSwitchOptical::setThreshold(thresholdBefore);
	directFrameBegin(this->directTxSeq++, DIRECT_SURVEY_END);
	msgpack::writeArraySize4(directStream, 2);
	msgpack::writeBool(directStream, aborted);
	msgpack::writeString(directStream, aborted ? "survey aborted" : "survey complete");
	directFrameEnd();
}
#endif

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
