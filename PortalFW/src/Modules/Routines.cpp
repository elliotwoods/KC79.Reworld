#include "Routines.h"
#include "App.h"
#include "../Platform.h"

namespace Modules {
	//----------
	Routines::Routines(App * app)
	: app(app)
	{

	}

	//----------
	const char *
	Routines::getTypeName() const
	{
		return "Routines";
	}

	//----------
	void
	Routines::startup()
	{
		auto exception = this->init(MotionControl::MeasureRoutineSettings());
		if(exception) {
			log(exception);
		}
	}

	//----------
	Exception
	Routines::init(const MotionControl::MeasureRoutineSettings & settings)
	{
		// create moduleName
		char moduleName[100];
		sprintf(moduleName, "%s.init", this->getName());

		log(LogLevel::Status, moduleName, "begin");

		app->motionControlA->stop();
		app->motionControlB->stop();

		if(app->motionControlA->unjamRoutine(settings).report()) {
			return Exception(moduleName, "Fail on unjam A");
		}

		if(app->motionControlB->unjamRoutine(settings).report()) {
			return Exception(moduleName, "Fail on unjam B");
		}

		// if(app->motionControlA->tuneCurrentRoutine(settings).report()) {
		// 	return Exception(moduleName, "Fail on tuneCurrent A");
		// }

		// if(app->motionControlB->tuneCurrentRoutine(settings).report()) {
		// 	return Exception(moduleName, "Fail on tuneCurrent B");
		// }

		if(this->calibrate(settings).report()) {
			return Exception(moduleName, "Fail on calibrate");
		}

		log(LogLevel::Status, moduleName, "end");

		return Exception::None();
	}

	//----------
	Exception
	Routines::unjam(const MotionControl::MeasureRoutineSettings & settings)
	{
				// create moduleName
		char moduleName[100];
		sprintf(moduleName, "%s.unjam", this->getName());

		log(LogLevel::Status, moduleName, "begin");

		app->motionControlA->stop();
		app->motionControlB->stop();

		if(app->motionControlA->unjamRoutine(settings).report()) {
			return Exception(moduleName, "Fail on unjam A");
		}

		if(app->motionControlB->unjamRoutine(settings).report()) {
			return Exception(moduleName, "Fail on unjam B");
		}

		log(LogLevel::Status, moduleName, "end");

		return Exception::None();
	}

	//----------
	Exception
	Routines::calibrate(const MotionControl::MeasureRoutineSettings & settings)
	{
		// create moduleName
		char moduleName[100];
		sprintf(moduleName, "%s.calibrate", this->getName());

		log(LogLevel::Status, moduleName, "begin");

		app->motionControlA->stop();
		app->motionControlB->stop();

		if(app->motionControlA->measureBacklashRoutine(settings).report()) {
			return Exception(moduleName, "Fail on measure backlash A");
		}
		if(app->motionControlA->homeRoutine(settings).report()) {
			return Exception(moduleName, "Fail on home A");
		}

		if(app->motionControlB->measureBacklashRoutine(settings).report()) {
			return Exception(moduleName, "Fail on measure backlash B");
		}
		if(app->motionControlB->homeRoutine(settings).report()) {
			return Exception(moduleName, "Fail on home B");
		}

		log(LogLevel::Status, moduleName, "end");

		return Exception::None();
	}

		//----------
	Exception
	Routines::home(const MotionControl::MeasureRoutineSettings & settings)
	{
		// create a moduleName
		char moduleName[100];
		sprintf(moduleName, "%s.home", this->getName());

		log(LogLevel::Status, moduleName, "begin");

		app->motionControlA->stop();
		app->motionControlB->stop();

		if(app->motionControlA->homeRoutine(settings).report()) {
			return Exception(moduleName, "Fail on home A");
		}
		if(app->motionControlB->homeRoutine(settings).report()) {
			return Exception(moduleName, "Fail on home B");
		}

		log(LogLevel::Status, moduleName, "end");

		return Exception::None();
	}

	//----------
	void
	Routines::flashLEDs(uint16_t period, uint16_t count)
	{
		for (uint16_t i = 0; i < count; i++)
		{
			log(LogLevel::Status, this->getName(), "LED Flash");
			digitalWrite(LED_INDICATOR, HIGH);
			digitalWrite(LED_HEARTBEAT, HIGH);
			delay(period / 2);
			digitalWrite(LED_INDICATOR, LOW);
			digitalWrite(LED_HEARTBEAT, LOW);
			delay(period / 2);

			App::updateFromRoutine();
		}
	}
}