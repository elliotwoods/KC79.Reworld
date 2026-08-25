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
		this->persistentIdentity = PersistentStorage::readIdentity();
		this->persistentSettings = PersistentStorage::readSettings();

		// The persisted current can raise the operating current, never lower it.
		//
		// 250 mA is what a module runs at now (MOTORDRIVERSETTINGS_DEFAULT_CURRENT). Records
		// written before that decision still ask for 150, and honouring them would leave
		// exactly the boards that have been through provisioning running gentler than a virgin
		// one -- and would quietly re-arm the current-recovery retry that this change exists to
		// make unnecessary. Observed on the bench: a board came up "Operating Current: 150 mA
		// (flash-b)" with the new default compiled in, because the record won.
		//
		// Clamped here rather than at the point of use so that the boot log, the RS485 settings
		// report and the motor all agree about one number.
		{
			const uint16_t defaultMa =
				(uint16_t) (MOTORDRIVERSETTINGS_DEFAULT_CURRENT * 1000.0f + 0.5f);
			if(this->persistentSettings.operatingCurrentMa < defaultMa) {
				char line[110];
				sprintf(line, "Operating Current: raising persisted %u mA to %u mA\r\n"
					, (unsigned int) this->persistentSettings.operatingCurrentMa
					, (unsigned int) defaultMa);
				Logger::X().printRaw(line);
				this->persistentSettings.operatingCurrentMa = defaultMa;
			}
		}

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

		MotorDriverSettings::Config motorSettingsConfig;
		motorSettingsConfig.initialCurrent = (float) this->persistentSettings.operatingCurrentMa / 1000.0f;
		this->motorDriverSettings = new MotorDriverSettings(motorSettingsConfig);
		this->motorDriverSettings->setup();

		{
			char line[96];
			sprintf(line, "Provision Serial: %lu\r\n", (unsigned long) this->getProvisionSerial());
			Logger::X().printRaw(line);
			sprintf(line, "Firmware Version: %s\r\n", PORTAL_VERSION_STRING);
			Logger::X().printRaw(line);
			sprintf(line, "Operating Current: %u mA (%s)\r\n"
				, (unsigned int) this->persistentSettings.operatingCurrentMa
				, PersistentStorage::sourceName(this->persistentSettings.source));
			Logger::X().printRaw(line);
			sprintf(line, "Full-current Home Recovery: %s\r\n"
				, this->persistentSettings.fullCurrentHomeRecovery ? "enabled" : "disabled");
			Logger::X().printRaw(line);
			sprintf(line, "Optical Calibration: A=%s T%u/W%ld; B=%s T%u/W%ld\r\n"
				, this->persistentSettings.axisACalibrationValid ? "flash" : "default"
				, (unsigned int) this->persistentSettings.axisAThreshold
				, (long) this->persistentSettings.axisAWidth
				, this->persistentSettings.axisBCalibrationValid ? "flash" : "default"
				, (unsigned int) this->persistentSettings.axisBThreshold
				, (long) this->persistentSettings.axisBWidth);
			Logger::X().printRaw(line);
		}

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
#ifndef HOME_SWITCH_LEGACY
		if(this->persistentSettings.axisACalibrationValid) {
			this->motionControlA->restoreOpticalCalibration(
				this->persistentSettings.axisAThreshold, this->persistentSettings.axisAWidth);
		}
#endif

		this->motionControlB = new MotionControl(*this->motorDriverSettings, *this->motorDriverB, *this->homeSwitchB);
		this->motionControlB->setup();
#ifndef HOME_SWITCH_LEGACY
		if(this->persistentSettings.axisBCalibrationValid) {
			this->motionControlB->restoreOpticalCalibration(
				this->persistentSettings.axisBThreshold, this->persistentSettings.axisBWidth);
		}
#endif

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

		// Out of the routine, so the next one starts from the neutral signal. LEDs::update()
		// below owns the pins from here.
		this->routineSignal = RoutineSignal::Normal;

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
	void
	App::setRoutineSignal(RoutineSignal value)
	{
		App::instance->routineSignal = value;
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

		// Alternate flashes -- double rate while startup's check is running. See
		// App::RoutineSignal.
		{
			const uint32_t period = App::instance->routineSignal == RoutineSignal::Checking
				? 250
				: 500;
			auto state = (bool) (millis() % period < period / 2);
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

	uint32_t App::getProvisionSerial() const {
		return this->persistentIdentity.valid ? this->persistentIdentity.serial : 0;
	}

	uint16_t App::getOperatingCurrentMa() const {
		return this->persistentSettings.operatingCurrentMa;
	}

	bool App::getFullCurrentHomeRecovery() const {
		return this->persistentSettings.fullCurrentHomeRecovery;
	}

	bool App::persistOperatingSettings(uint16_t currentMa, bool recovery) {
		if(!PersistentStorage::writeSettings(currentMa, recovery)) return false;
		this->persistentSettings = PersistentStorage::readSettings();
		this->motorDriverSettings->setCurrent((float) currentMa / 1000.0f);
		return true;
	}

	bool App::persistOpticalCalibration(MotionControl * axis) {
#ifdef HOME_SWITCH_LEGACY
		(void) axis;
		return true;
#else
		if(!axis || axis->getOpticalThreshold() <= 0 || axis->getOpticalWidth() <= 0) return false;
		PersistentStorage::Settings desired = this->persistentSettings;
		desired.opticalCalibrationVersion = 1;
		if(axis == this->motionControlA) {
			if(desired.opticalCalibrationVersion == this->persistentSettings.opticalCalibrationVersion
				&& this->persistentSettings.axisACalibrationValid
				&& abs((int) this->persistentSettings.axisAThreshold - axis->getOpticalThreshold()) < 2
				&& abs(this->persistentSettings.axisAWidth - axis->getOpticalWidth()) * 10
					<= this->persistentSettings.axisAWidth) return true;
			desired.axisACalibrationValid = true;
			desired.axisAThreshold = (uint8_t) axis->getOpticalThreshold();
			desired.axisAWidth = axis->getOpticalWidth();
		} else if(axis == this->motionControlB) {
			if(desired.opticalCalibrationVersion == this->persistentSettings.opticalCalibrationVersion
				&& this->persistentSettings.axisBCalibrationValid
				&& abs((int) this->persistentSettings.axisBThreshold - axis->getOpticalThreshold()) < 2
				&& abs(this->persistentSettings.axisBWidth - axis->getOpticalWidth()) * 10
					<= this->persistentSettings.axisBWidth) return true;
			desired.axisBCalibrationValid = true;
			desired.axisBThreshold = (uint8_t) axis->getOpticalThreshold();
			desired.axisBWidth = axis->getOpticalWidth();
		} else {
			return false;
		}
		{
			char message[120];
			sprintf(message, "optical settings write: axis=%c T=%u W=%ld beforeGen=%lu beforeMask=%u"
				, axis == this->motionControlA ? 'A' : 'B'
				, (unsigned int) (axis == this->motionControlA
					? desired.axisAThreshold : desired.axisBThreshold)
				, (long) (axis == this->motionControlA ? desired.axisAWidth : desired.axisBWidth)
				, (unsigned long) this->persistentSettings.generation
				, (unsigned int) ((this->persistentSettings.axisACalibrationValid ? 1 : 0)
					| (this->persistentSettings.axisBCalibrationValid ? 2 : 0)));
			log(LogLevel::Status, "PersistentStorage", message);
		}
		if(!PersistentStorage::writeSettings(desired)) return false;
		this->persistentSettings = PersistentStorage::readSettings();
		{
			char message[110];
			sprintf(message, "optical settings committed: gen=%lu mask=%u source=%s"
				, (unsigned long) this->persistentSettings.generation
				, (unsigned int) ((this->persistentSettings.axisACalibrationValid ? 1 : 0)
					| (this->persistentSettings.axisBCalibrationValid ? 2 : 0))
				, PersistentStorage::sourceName(this->persistentSettings.source));
			log(LogLevel::Status, "PersistentStorage", message);
		}
		return this->persistentSettings.valid
			&& (axis == this->motionControlA
				? this->persistentSettings.axisACalibrationValid
				: this->persistentSettings.axisBCalibrationValid);
#endif
	}

	//----------
	void
	App::reportStatus(msgpack::Serializer &serializer)
	{
		serializer.beginMap(5);
		{
			serializer << "app";
			{
				serializer.beginMap(7);
				{
					serializer << "upTime" << millis();
					serializer << "version" << PORTAL_VERSION_STRING;
					serializer << "provisionSerial" << this->getProvisionSerial();
					serializer << "settingsVersion" << (uint32_t) 2;
					serializer << "operatingCurrentMa" << this->getOperatingCurrentMa();
					serializer << "fullCurrentHomeRecovery" << this->getFullCurrentHomeRecovery();
					serializer << "settingsSource" << PersistentStorage::sourceName(this->persistentSettings.source);
				}
			}

			serializer << "mca";
			this->motionControlA->reportStatus(serializer);

			serializer << "mcb";
			this->motionControlB->reportStatus(serializer);

			serializer << "logger";
			Logger::X().reportStatus(serializer);

			serializer << "settings";
			serializer.beginMap(9);
			{
				serializer << "version" << (uint32_t) 2;
				serializer << "operatingCurrentMa" << this->getOperatingCurrentMa();
				serializer << "fullCurrentHomeRecovery" << this->getFullCurrentHomeRecovery();
				serializer << "source" << PersistentStorage::sourceName(this->persistentSettings.source);
				serializer << "opticalCalibrationVersion" << this->persistentSettings.opticalCalibrationVersion;
				serializer << "axisAThreshold" << this->persistentSettings.axisAThreshold;
				serializer << "axisAWidth" << this->persistentSettings.axisAWidth;
				serializer << "axisBThreshold" << this->persistentSettings.axisBThreshold;
				serializer << "axisBWidth" << this->persistentSettings.axisBWidth;
			}
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

		else if(strcmp(key, "settingsRead") == 0) {
			if(!msgpack::readNil(stream)) return false;
			if(RS485::replyAllowed()) rs485->sendStatusReport();
			return true;
		}

		else if(strcmp(key, "settingsWrite") == 0) {
			size_t count;
			if(!msgpack::readArraySize(stream, count) || count < 3) return false;
			uint32_t version;
			uint16_t currentMa;
			bool recovery;
			if(!msgpack::readInt<uint32_t>(stream, version)
				|| !msgpack::readInt<uint16_t>(stream, currentMa)
				|| !msgpack::readBool(stream, recovery)) return false;
			if(version != 1 || currentMa < 50 || currentMa > 250) return false;
			if(!RS485::checkChecksum()) return false;
			return this->persistOperatingSettings(currentMa, recovery);
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
