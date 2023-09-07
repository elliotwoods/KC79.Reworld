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
#endif

#ifndef GUI_DISABLED
		this->gui->update();
#endif

		// Heartbeat LED
		{
			auto isCalibrated = this->motionControlA->isHomeCalibrated()
				&& this->motionControlA->isBacklashCalibrated()
				&& this->motionControlB->isHomeCalibrated()
				&& this->motionControlB->isBacklashCalibrated();
			
			if(isCalibrated) {
				// Slow heartbeat
				analogWrite(LED_HEARTBEAT, (millis() % 2000) / 128);
			} else {
				// Fast heartbeat
				analogWrite(LED_HEARTBEAT, (millis() % 512) / 32);
			}
		}

		// Indicate if a motor driver is enabled
		if(this->motorIndicatorEnabled) {
			digitalWrite(LED_INDICATOR, this->motorDriverA->getEnabled() || this->motorDriverB->getEnabled());
		}		
		// Refresh the watchdog counter
		LL_IWDG_ReloadCounter(IWDG);

		// set distant target if no signal received
		if(!this->rs485->hasAnySignalBeenReceived()) {
			this->motionControlA->setTargetPosition(this->motionControlA->getMicrostepsPerPrismRotation() * 1000);
			this->motionControlB->setTargetPosition(this->motionControlA->getMicrostepsPerPrismRotation() * 1000);
		}
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
			log(LogLevel::Status, "Exiting routine");
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

			// Special 2-axis move message
			size_t arraySize;
			if (!msgpack::readArraySize(stream, arraySize))
			{
				return false;
			}
			if (arraySize >= 1)
			{
				Steps position;
				if (!msgpack::readInt<int32_t>(stream, position))
				{
					return false;
				}
				this->motionControlA->setTargetPositionWithMotionFiltering(position);
			}
			if (arraySize >= 2)
			{
				Steps position;
				if (!msgpack::readInt<int32_t>(stream, position))
				{
					return false;
				}
				this->motionControlB->setTargetPositionWithMotionFiltering(position);
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

		else if (strcmp(key, "motorIndicatorEnabled") == 0) {
			bool value;
			if(!msgpack::readBool(stream, value)) {
				return false;
			}
			this->motorIndicatorEnabled = value;
			return true;
		}
		else if (strcmp(key, "escapeFromRoutine") == 0) {
			if(!msgpack::readNil(stream)) {
				return false;
			}
			this->escapeFromRoutine();
			return true;
		}

		else if (strcmp(key, "motorDriverIndicator") == 0) {
			bool value;
			if(!msgpack::readBool(stream, value)) {
				return false;
			}
			this->motorIndicatorEnabled = value;
			return true;
		}

		else if (strcmp(key, "reset") == 0)
		{
			NVIC_SystemReset();
		}

		return false;
	}
}