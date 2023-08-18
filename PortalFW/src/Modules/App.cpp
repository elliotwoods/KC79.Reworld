#include "App.h"
#include <Arduino.h>

namespace Modules {
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
		if(STARTUP_ENABLED) {
			this->initRoutine(3);
		}
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
		digitalWrite(PB3
			, this->motorDriverA->getEnabled() || this->motorDriverB->getEnabled());
	}
	
	//----------
	void
	App::reportStatus(msgpack::Serializer& serializer)
	{
		serializer.beginMap(3);
		{
			serializer << "mca";
			this->motionControlA->reportStatus(serializer);

			serializer << "mcb";
			this->motionControlB->reportStatus(serializer);

			serializer << "logger";
			Logger::X().reportStatus(serializer);
		}
	}

	//----------
	void
	App::initRoutine(uint8_t tryCount)
	{
		MotionControl::MeasureRoutineSettings settings;

		auto tryNTimes = [this, &tryCount](const std::function<Exception()> & action) {
			for(uint8_t tries = 0; tries < tryCount; tries++) {
				auto result = action();
				if(result) {
					log(LogLevel::Error, result.what());
				}
				else {
					return true;
				}
			}
			return false;
		};
		
		bool success = true;
		success &= tryNTimes([this, &settings]() {
			return this->motionControlA->measureBacklashRoutine(settings);
		});
		success &= tryNTimes([this, &settings]() {
			return this->motionControlA->homeRoutine(settings);
		});

		success &= tryNTimes([this, &settings]() {
			return this->motionControlB->measureBacklashRoutine(settings);
		});
		success &= tryNTimes([this, &settings]() {
			return this->motionControlB->homeRoutine(settings);
		});
		if(success) {
			log(LogLevel::Status, "Init OK");
		}
		else {
			log(LogLevel::Error, "Init fail");
		}
	}

	//----------
	bool
	App::processIncomingByKey(const char * key, Stream & stream)
	{
		if(strcmp(key, "id") == 0) {
			return this->id->processIncoming(stream);
		}
		
		if(strcmp(key, "motorDriverSettings") == 0) {
			return this->motorDriverSettings->processIncoming(stream);
		}

		if(strcmp(key, "motorDriverA") == 0) {
			return this->motorDriverA->processIncoming(stream);
		}
		if(strcmp(key, "motorDriverB") == 0) {
			return this->motorDriverB->processIncoming(stream);
		}

		if(strcmp(key, "motionControlA") == 0) {
			return this->motionControlA->processIncoming(stream);
		}
		if(strcmp(key, "motionControlB") == 0) {
			return this->motionControlB->processIncoming(stream);
		}
		if(strcmp(key, "poll") == 0) {
			if(!msgpack::readNil(stream)) {
				return false;
			}
			
			// Now it's the end of the input stream and we're ready to write

			rs485->sendStatusReport();
			return true;
		}
		if(strcmp(key, "init") == 0) {
			msgpack::DataType dataType;
			if(!msgpack::getNextDataType(stream, dataType)) {
				return false;
			}
			if(dataType == msgpack::DataType::Nil) {
				this->initRoutine(1);
				return true;
			}
			else if(msgpack::isInt(dataType)) {
				uint8_t tryCount;
				if(!msgpack::readInt<uint8_t>(stream, tryCount)) {
					return false;
				}
				this->initRoutine(tryCount);
				return true;
			}
			return false;
		}
		
		if(strcmp(key, "reset") == 0) {
			NVIC_SystemReset();
		}
		if(strcmp(key, "FW") == 0) {
			// Firmware announce packet - go to bootloader
			NVIC_SystemReset();
		}
		

		return false;
	}
}