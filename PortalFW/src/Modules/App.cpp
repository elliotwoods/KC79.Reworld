#include "App.h"
#include <Arduino.h>
#include "../Version.h"

#define INDICATOR_LED PB3
namespace Modules
{
	//----------
	const char *
	App::getTypeName() const
	{
		return "App";
	}

	//----------
	void
	App::setup()
	{
		Logger::setup();

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

		// Calibrate self on startup
#ifndef STARTUP_INIT_DISABLED
		this->initRoutine(3);
#endif
	}

	//----------
	void
	App::update()
	{
		this->id->update();
		this->rs485->update();
		this->motorDriverSettings->update();
		this->motorDriverA->update();
		this->motorDriverB->update();
		this->homeSwitchA->update();
		this->homeSwitchB->update();
		this->motionControlA->update();
		this->motionControlB->update();

#ifndef GUI_DISABLED
		this->gui->update();
#endif

		// OLD HACK TO JUST RUN MOTORS AND DO THINGS
		// this->motorDriverSettings->setCurrent(0.15f);
		// this->motorDriverA->testRoutine();
		// this->motorDriverB->testRoutine();
		// this->motorDriverSettings->setCurrent(0.05f);

		// Indicate if either driver is enabled
		digitalWrite(INDICATOR_LED, this->motorDriverA->getEnabled() || this->motorDriverB->getEnabled());
	}

	//----------
	void
	App::reportStatus(msgpack::Serializer &serializer)
	{
		serializer.beginMap(4);
		{
			serializer << "app";
			{
				serializer.beginMap(3);
				{
					serializer << "upTime" << millis();
					serializer << "version" << PORTAL_VERSION_STRING;
					serializer << "calibrated" << this->calibrated;
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
	tryNTimes(const std::function<Exception()> &action, uint8_t tryCount)
	{
		for (uint8_t tries = 0; tries < tryCount; tries++)
		{
			auto result = action();
			if (result)
			{
				log(LogLevel::Error, result.what());
			}
			else
			{
				return true;
			}
		}
		return false;
	}
	//----------
	bool
	App::initRoutine(uint8_t tryCount)
	{
		log(LogLevel::Status, "initRoutine : begin");

		bool success = true;
		MotionControl::MeasureRoutineSettings settings;

		// Walk both by 1/2 a rotation
		success &= tryNTimes([this, &settings]()
							 { return this->walkBackAndForthRoutine(settings); },
							 tryCount);

		success &= this->calibrateRoutine(tryCount);

		if (success)
		{
			log(LogLevel::Status, "initRoutine : OK");
		}
		else
		{
			log(LogLevel::Error, "initRoutine : fail");
		}

		return success;
	}

	//----------
	bool
	App::calibrateRoutine(uint8_t tryCount)
	{
		log(LogLevel::Status, "calibrateRoutine : begin");

		bool success = true;
		MotionControl::MeasureRoutineSettings settings;

		success &= tryNTimes([this, &settings]()
							 { return this->motionControlA->measureBacklashRoutine(settings); },
							 tryCount);
		success &= tryNTimes([this, &settings]()
							 { return this->motionControlA->homeRoutine(settings); },
							 tryCount);

		success &= tryNTimes([this, &settings]()
							 { return this->motionControlB->measureBacklashRoutine(settings); },
							 tryCount);
		success &= tryNTimes([this, &settings]()
							 { return this->motionControlB->homeRoutine(settings); },
							 tryCount);

		if (success)
		{
			this->calibrated = true;
			log(LogLevel::Status, "calibrateRoutine : OK");
		}
		else
		{
			log(LogLevel::Error, "calibrateRoutine : fail");
		}

		return success;
	}

	//----------
	void
	App::flashLEDsRoutine(uint16_t period, uint16_t count)
	{
		for (uint16_t i = 0; i < count; i++)
		{
			log(LogLevel::Status, "LED Flash");
			digitalWrite(INDICATOR_LED, HIGH);
			delay(period / 2);
			digitalWrite(INDICATOR_LED, LOW);
			delay(period / 2);
		}
	}

	//----------
	bool
	App::processIncomingByKey(const char *key, Stream &stream)
	{
		if (strcmp(key, "m") == 0)
		{
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
				this->motionControlA->setTargetPosition(position);
			}
			if (arraySize >= 2)
			{
				Steps position;
				if (!msgpack::readInt<int32_t>(stream, position))
				{
					return false;
				}
				this->motionControlB->setTargetPosition(position);
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

		else if (strcmp(key, "poll") == 0)
		{
			if (!msgpack::readNil(stream))
			{
				return false;
			}

			// Now it's the end of the input stream and we're ready to write

			rs485->sendStatusReport();
			return true;
		}
		else if (strcmp(key, "p") == 0)
		{
			// Miniature poll (positions only)

			if (!msgpack::readNil(stream))
			{
				return false;
			}

			rs485->sendPositions();
			return true;
		}

		else if (strcmp(key, "init") == 0)
		{
			msgpack::DataType dataType;
			uint8_t tryCount = 1;
			if (!msgpack::getNextDataType(stream, dataType))
			{
				return false;
			}
			if (dataType == msgpack::DataType::Nil)
			{
				msgpack::readNil(stream);
			}
			else if (msgpack::isInt(dataType))
			{
				if (!msgpack::readInt<uint8_t>(stream, tryCount))
				{
					return false;
				}
				return true;
			}
			else
			{
				return false;
			}
			RS485::sendACKEarly(true);
			this->initRoutine(tryCount);
		}
		else if (strcmp(key, "calibrate") == 0)
		{
			msgpack::DataType dataType;
			uint8_t tryCount = 1;
			if (!msgpack::getNextDataType(stream, dataType))
			{
				return false;
			}
			if (dataType == msgpack::DataType::Nil)
			{
				msgpack::readNil(stream);
			}
			else if (msgpack::isInt(dataType))
			{
				if (!msgpack::readInt<uint8_t>(stream, tryCount))
				{
					return false;
				}
				return true;
			}
			else
			{
				return false;
			}
			RS485::sendACKEarly(true);
			this->calibrateRoutine(tryCount);
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

			this->flashLEDsRoutine(period, count);
			return true;
		}

		else if (strcmp(key, "reset") == 0)
		{
			NVIC_SystemReset();
		}

		return false;
	}

	//----------
	Exception
	App::walkBackAndForthRoutine(const MotionControl::MeasureRoutineSettings &settings)
	{
		auto routineStart = millis();
		auto routineDeadline = routineStart + (uint32_t)settings.timeout_s * 1000;

		// Instruct move
		auto moveEnd = this->motionControlA->getMicrostepsPerPrismRotation() / 2;
		this->motionControlA->setTargetPosition(moveEnd + this->motionControlA->getPosition());
		this->motionControlB->setTargetPosition(moveEnd + this->motionControlA->getPosition());

		// Get default values (for timeout)
		MotionControl::MeasureRoutineSettings measureRoutineSettings;

		// Wait for move
		log(LogLevel::Status, "Walk routine CW");
		while (this->motionControlA->getPosition() < this->motionControlA->getTargetPosition() || this->motionControlB->getPosition() < this->motionControlB->getTargetPosition())
		{
			motionControlA->update();
			motionControlB->update();
			HAL_Delay(1);

			if (millis() > routineDeadline)
			{
				return Exception::Timeout();
			}
		}

		// Instruct move back to 0
		this->motionControlA->setTargetPosition(0);
		this->motionControlB->setTargetPosition(0);

		// Wait for move
		log(LogLevel::Status, "Walk routine CCW");
		while (this->motionControlA->getPosition() > this->motionControlA->getTargetPosition() || this->motionControlB->getPosition() > this->motionControlB->getTargetPosition())
		{
			motionControlA->update();
			motionControlB->update();
			HAL_Delay(1);

			if (millis() > routineDeadline)
			{
				return Exception::Timeout();
			}
		}

		this->motionControlA->stop();
		this->motionControlB->stop();

		return Exception::None();
	}
}