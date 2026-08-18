#include "App.h"
#include <Arduino.h>

#include "../Version.h"
#include "../Platform.h"

#include "stm32g0xx_ll_iwdg.h"

namespace Modules
{
	//----------
	App * App::instance = nullptr;

	//----------
	const char *
	App::getTypeName() const
	{
		return "App";
	}

	//----------
	App::App()
	{
		this->instance = this;
	}

	//----------
	App &
	App::X()
	{
		return *App::instance;
	}

	//----------
	void
	App::setup()
	{
		Logger::X().setup();

#ifndef GUI_DISABLED
		this->gui = new GUI();
		this->gui->setup();
#endif

		this->id = new ID();
		this->id->setup();

		this->rs485 = new RS485(this);
		this->rs485->setup();

		this->leds = new LEDs();
		this->leds->setup();

		this->motorDriverSettings = new MotorDriverSettings(MotorDriverSettings::Config());
		this->motorDriverSettings->setup();

		this->motorDriverA = new MotorDriver(MotorDriver::Config::MotorA());
		this->motorDriverA->setup();

		this->motorDriverB = new MotorDriver(MotorDriver::Config::MotorB());
		this->motorDriverB->setup();

		this->homeSwitchA = new HomeSwitch(HomeSwitch::Config::A());
		this->homeSwitchA->setup();

		this->homeSwitchB = new HomeSwitch(HomeSwitch::Config::B());
		this->homeSwitchB->setup();

		this->motionControlA = new MotionControl(*this->motorDriverSettings, *this->motorDriverA, *this->homeSwitchA);
		this->motionControlA->setup();

		this->motionControlB = new MotionControl(*this->motorDriverSettings, *this->motorDriverB, *this->homeSwitchB);
		this->motionControlB->setup();

		this->routines = new Routines(this);

		this->keyframeMotionControl = new KeyframeMotionControl();
		
		// Calibrate self on startup
#ifndef STARTUP_INIT_DISABLED
		this->routines->startup();
#endif
	}

	//----------
	void
	App::update()
	{
		// reset these flags
		this->isInsideRoutine = false;
		this->shouldEscapeFromRoutine = false;

		// reset the indicator LED
		digitalWrite(LED_INDICATOR, LOW);

		Logger::X().update();

#ifndef UPDATE_DISABLED
		this->id->update();
		this->rs485->update();
		this->motorDriverSettings->update();
		this->motorDriverA->update();
		this->motorDriverB->update();
		this->homeSwitchA->update();
		this->homeSwitchB->update();
		this->motionControlA->update();
		this->motionControlB->update();
		this->routines->update();
		this->keyframeMotionControl->update();
#endif

#ifndef GUI_DISABLED
		this->gui->update();
#endif

		this->leds->update();

		// Refresh the watchdog counter
		LL_IWDG_ReloadCounter(IWDG);

		// set distant target if no signal received
		// if(!this->rs485->hasAnySignalBeenReceived()) {
		// 	this->motionControlA->setTargetPosition(this->motionControlA->getMicrostepsPerPrismRotation() * 1024 * 8);
		// 	this->motionControlB->setTargetPosition(this->motionControlA->getMicrostepsPerPrismRotation() * 1024 * 8);
		// }
	}

	//---------
	bool
	App::updateFromRoutine()
	{
		// Make sure we know we're inside a routine
		App::instance->isInsideRoutine = true;

		// Update logger (e.g. dump messages on request)
		Logger::X().update();

		// Process RS485 messages
		App::instance->rs485->update();

		// Perform ID updates
		App::instance->id->update();

		// Feed the watchdog
		LL_IWDG_ReloadCounter(IWDG);

		// Alternate flashes
		{
			auto state = (bool) (millis() % 500 < 250);
			digitalWrite(LED_INDICATOR, state ? HIGH : LOW);
			digitalWrite(LED_HEARTBEAT, state ? LOW : HIGH);
		}

#ifndef GUI_DISABLED
		// Update GUI
		App::instance->gui->update();
#endif

		if(App::instance->shouldEscapeFromRoutine) {
			log(LogLevel::Status, "App", "Exiting routine");
			return true;
		}
		else {
			return false;
		}
	}

	//---------
	void
	App::escapeFromRoutine()
	{
		this->shouldEscapeFromRoutine = true;
	}

	//---------
	bool
	App::getShouldEscapeFromRoutine()
	{
		return App::instance->shouldEscapeFromRoutine;
	}

	//----------
	MotionControl *
	App::getMotionControl(uint8_t index)
	{
		if(index == 0) {
			return this->motionControlA;
		}
		else {
			return this->motionControlB;
		}
	}

	//----------
	void
	App::reportStatus(msgpack::Serializer &serializer)
	{
		serializer.beginMap(4);
		{
			serializer << "app";
			{
				serializer.beginMap(2);
				{
					serializer << "upTime" << millis();
					serializer << "version" << PORTAL_VERSION_STRING;
				}
			}

			serializer << "mca";
			this->motionControlA->reportStatus(serializer);

			serializer << "mcb";
			this->motionControlB->reportStatus(serializer);

			serializer << "logger";
			Logger::X().reportStatus(serializer);
		}
	}

	//----------
	bool
	App::processIncomingByKey(const char *key, Stream &stream)
	{
		if (strcmp(key, "poll") == 0)
		{
			// Fully read the input stream
			if (!msgpack::readNil(stream))
			{
				return false;
			}

			// Now it's the end of the input stream and we're ready to write

#ifndef POLL_DISABLED
			if(RS485::replyAllowed()) {
				rs485->sendStatusReport();
			}
#endif
			return true;
		}

		else if (strcmp(key, "m") == 0)
		{
			// Can't do whilst already inside routine
			if(this->isInsideRoutine) {
				// Don't report a malformed message, but rest of message will be ignored
				return true;
			}

			// Special 2-axis move message. Positions are staged in locals and only applied
			// after checkChecksum() -- the array is the entire body (nothing follows it), so
			// once both elements are read the stream is exactly at the trailer.
			size_t arraySize;
			if (!msgpack::readArraySize(stream, arraySize))
			{
				return false;
			}
			bool haveA = false, haveB = false;
			Steps positionA = 0, positionB = 0;
			if (arraySize >= 1)
			{
				if (!msgpack::readInt<int32_t>(stream, positionA))
				{
					return false;
				}
				haveA = true;
			}
			if (arraySize >= 2)
			{
				if (!msgpack::readInt<int32_t>(stream, positionB))
				{
					return false;
				}
				haveB = true;
			}

			if(!RS485::checkChecksum()) {
				return false;
			}
			if(haveA) {
				this->motionControlA->setTargetPositionWithMotionFiltering(positionA);
			}
			if(haveB) {
				this->motionControlB->setTargetPositionWithMotionFiltering(positionB);
			}

			if(RS485::replyAllowed()) {
				rs485->sendPositions();
			}
			return true;
		}

		else if (strcmp(key, "id") == 0)
		{
			return this->id->processIncoming(stream);
		}

		else if (strcmp(key, "motorDriverSettings") == 0)
		{
			return this->motorDriverSettings->processIncoming(stream);
		}

		else if (strcmp(key, "motorDriverA") == 0)
		{
			return this->motorDriverA->processIncoming(stream);
		}
		else if (strcmp(key, "motorDriverB") == 0)
		{
			return this->motorDriverB->processIncoming(stream);
		}

		else if (strcmp(key, "motionControlA") == 0)
		{
			return this->motionControlA->processIncoming(stream);
		}
		else if (strcmp(key, "motionControlB") == 0)
		{
			return this->motionControlB->processIncoming(stream);
		}

		else if (strcmp(key, "p") == 0)
		{
			// Miniature poll (positions only)

			if (!msgpack::readNil(stream))
			{
				return false;
			}

			if(RS485::replyAllowed()) {
				rs485->sendPositions();
			}

			return true;
		}

		else if (strcmp(key, "init") == 0)
		{
			// Can't do whilst already inside routine
			if(this->isInsideRoutine) {
				// Don't report a malformed message, but rest of message will be ignored
				return true;
			}

			MotionControl::MeasureRoutineSettings settings;
			if(!MotionControl::readMeasureRoutineSettings(stream, settings)) {
				return false;
			}

			// Nil or the settings array is the entire body -- the stream is exactly at the
			// trailer here. Gated before sendACKEarly(), not just before the routine itself:
			// the early ACK is its own side effect (it tells the Router "started"), and
			// shouldn't fire for a corrupted frame either.
			if(!RS485::checkChecksum()) {
				return false;
			}

			RS485::sendACKEarly(true);

			this->routines->init(settings);
			return true;
		}
		else if (strcmp(key, "calibrate") == 0)
		{
			// Can't do whilst already inside routine
			if(this->isInsideRoutine) {
				// Don't report a malformed message, but rest of message will be ignored
				return true;
			}

			MotionControl::MeasureRoutineSettings settings;
			if(!MotionControl::readMeasureRoutineSettings(stream, settings)) {
				return false;
			}

			if(!RS485::checkChecksum()) {
				return false;
			}

			RS485::sendACKEarly(true);

			this->routines->calibrate(settings);
			return true;
		}
		else if (strcmp(key, "home") == 0)
		{
			// Can't do whilst already inside routine
			if(this->isInsideRoutine) {
				// Don't report a malformed message, but rest of message will be ignored
				return true;
			}

			MotionControl::MeasureRoutineSettings settings;
			if(!MotionControl::readMeasureRoutineSettings(stream, settings)) {
				return false;
			}

			if(!RS485::checkChecksum()) {
				return false;
			}

			RS485::sendACKEarly(true);

			this->routines->home(settings);
			return true;
		}
		else if (strcmp(key, "unjam") == 0)
		{
			// Can't do whilst already inside routine
			if(this->isInsideRoutine) {
				// Don't report a malformed message, but rest of message will be ignored
				return true;
			}

			MotionControl::MeasureRoutineSettings settings;
			if(!MotionControl::readMeasureRoutineSettings(stream, settings)) {
				return false;
			}

			if(!RS485::checkChecksum()) {
				return false;
			}

			RS485::sendACKEarly(true);

			this->routines->unjam(settings);
			return true;
		}
		if (strcmp(key, "flashLED") == 0)
		{
			msgpack::DataType dataType;
			if (!msgpack::getNextDataType(stream, dataType))
			{
				return false;
			}

			uint16_t period = 500;
			uint16_t count = 5;

			if (dataType == msgpack::DataType::Nil)
			{
				msgpack::readNil(stream);
			}
			else if (dataType == msgpack::DataType::Array)
			{
				size_t arraySize;

				if (!msgpack::readArraySize(stream, arraySize))
				{
					return false;
				}
				if (arraySize >= 1)
				{
					if (!msgpack::readInt<uint16_t>(stream, period))
					{
						return false;
					}
				}
				if (arraySize >= 2)
				{
					if (!msgpack::readInt<uint16_t>(stream, count))
					{
						return false;
					}
				}
			}
			else
			{
				return false;
			}

			this->routines->flashLEDs(period, count);
			return true;
		}

		else if (strcmp(key, "debugLightsEnabled") == 0) {
			bool value;
			if(!msgpack::readBool(stream, value)) {
				return false;
			}
			this->leds->setDebugLightsEnabled(value);
			return true;
		}

#ifndef HOME_SWITCH_LEGACY
		else if (strcmp(key, "homeThreshold") == 0) {
			int32_t value;
			if(!msgpack::readInt<int32_t>(stream, value)) {
				return false;
			}
			HomeSwitchOptical::setThreshold((uint8_t) value);
			return true;
		}
#endif

		else if (strcmp(key, "escapeFromRoutine") == 0) {
			if(!msgpack::readNil(stream)) {
				return false;
			}
			this->escapeFromRoutine();
			return true;
		}

		else if (strcmp(key, "reset") == 0)
		{
			if(!msgpack::readNil(stream)) {
				return false;
			}
			// Nil is the entire body -- nothing else follows it, so the stream is exactly at
			// the trailer here. No-ops (returns true) until verifyChecksumEnabled is turned
			// on -- see RS485::checkChecksum().
			if(!RS485::checkChecksum()) {
				return false;
			}
			NVIC_SystemReset();
		}

		else if (strcmp(key, "verifyChecksum") == 0) {
			// Toggle RS485::checkChecksum() actually verifying the trailing [seq, crc16]
			// rather than being a no-op -- see the flag's doc comment in RS485.h. Left off
			// until the Router side is confirmed to be sending the trailer on everything.
			bool value;
			if(!msgpack::readBool(stream, value)) {
				return false;
			}
			RS485::setVerifyChecksumEnabled(value);
			return true;
		}

		else if (strcmp(key, "keyframe") == 0)
		{
			if(this->isInsideRoutine) {
				return true;
			}

			return this->keyframeMotionControl->processIncoming(stream);
		}

		return false;
	}
}