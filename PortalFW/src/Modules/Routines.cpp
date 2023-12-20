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
		auto error = this->init(MotionControl::MeasureRoutineSettings());
		if(error) {
			log(LogLevel::Error, error.what());
		}
	}

	//----------
	Exception
	Routines::init(const MotionControl::MeasureRoutineSettings & settings)
	{
		log(LogLevel::Status, "Routines::startup : begin");

		app->motionControlA->stop();
		app->motionControlB->stop();

		if(app->motionControlA->unjamRoutine(settings).report()) {
			return Exception("Routines::startup : Fail on unjam A");
		}

		if(app->motionControlB->unjamRoutine(settings).report()) {
			return Exception("Routines::startup : Fail on unjam B");
		}

		// if(app->motionControlA->tuneCurrentRoutine(settings).report()) {
		// 	return Exception("Routines::startup : Fail on tuneCurrent A");
		// }

		// if(app->motionControlB->tuneCurrentRoutine(settings).report()) {
		// 	return Exception("Routines::startup : Fail on tuneCurrent B");
		// }

		if(this->calibrate(settings).report()) {
			return Exception("Routines::startup : Fail on calibrate");
		}

		log(LogLevel::Status, "Routines::startup : end");

		return Exception::None();
	}

	//----------
	Exception
	Routines::unjam(const MotionControl::MeasureRoutineSettings & settings)
	{
		log(LogLevel::Status, "Routines::unjam : begin");

		app->motionControlA->stop();
		app->motionControlB->stop();

		if(app->motionControlA->unjamRoutine(settings).report()) {
			return Exception("Routines::unjam : Fail on unjam A");
		}

		if(app->motionControlB->unjamRoutine(settings).report()) {
			return Exception("Routines::unjam : Fail on unjam B");
		}

		log(LogLevel::Status, "Routines::unjam : end");

		return Exception::None();
	}

	//----------
	Exception
	Routines::calibrate(const MotionControl::MeasureRoutineSettings & settings)
	{
		log(LogLevel::Status, "Routines::calibrate : begin");

		app->motionControlA->stop();
		app->motionControlB->stop();

		if(app->motionControlA->measureBacklashRoutine(settings).report()) {
			return Exception("Routines::calibrate : Fail on measure backlash A");
		}
		if(app->motionControlA->homeRoutine(settings).report()) {
			return Exception("Routines::calibrate : Fail on home A");
		}

		if(app->motionControlB->measureBacklashRoutine(settings).report()) {
			return Exception("Routines::calibrate : Fail on measure backlash B");
		}
		if(app->motionControlB->homeRoutine(settings).report()) {
			return Exception("Routines::calibrate : Fail on home B");
		}

		log(LogLevel::Status, "Routines::calibrate : end");

		return Exception::None();
	}

		//----------
	Exception
	Routines::home(const MotionControl::MeasureRoutineSettings & settings)
	{
		log(LogLevel::Status, "Routines::home : begin");

		app->motionControlA->stop();
		app->motionControlB->stop();

		if(app->motionControlA->homeRoutine(settings).report()) {
			return Exception("Routines::home : Fail on home A");
		}
		if(app->motionControlB->homeRoutine(settings).report()) {
			return Exception("Routines::home : Fail on home B");
		}

		log(LogLevel::Status, "Routines::home : end");

		return Exception::None();
	}

	//----------
	void
	Routines::flashLEDs(uint16_t period, uint16_t count)
	{
		for (uint16_t i = 0; i < count; i++)
		{
			log(LogLevel::Status, "LED Flash");
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