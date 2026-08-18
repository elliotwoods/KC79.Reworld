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
		
		auto startTime = millis();

		log(LogLevel::Status, moduleName, "begin");

		bool failedAnywhere = false;

		app->motionControlA->stop();
		app->motionControlB->stop();

		if(this->measureCycle(settings).report()) {
			if(settings.stopAllRoutinesIfOneFails) {
				Exception(moduleName, "Fail on measureCycle");
			}
			failedAnywhere = true;
		}

		if(this->calibrate(settings).report()) {
			if(settings.stopAllRoutinesIfOneFails) {
				return Exception(moduleName, "Fail on calibrate");
			}
			failedAnywhere = true;
		}

		auto endTime = millis();

		// Print the duration in seconds to 0.1s accuracy
		{
			char message[100];
			sprintf(message, "Duration: %ds", (endTime - startTime) / 1000);
			log(LogLevel::Status, moduleName, message);
		}

		// Move to zero position on both axes
		app->motionControlA->setTargetPosition(0);
		app->motionControlB->setTargetPosition(0);

		if(failedAnywhere) {
			return Exception(moduleName, "Fail");
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

		bool failedAnywhere = false;

		if(app->motionControlA->unjamRoutine(settings).report()) {
			if(settings.stopAllRoutinesIfOneFails) {
				return Exception(moduleName, "Fail on unjam A");
			}
			failedAnywhere = true;
		}

		if(app->motionControlB->unjamRoutine(settings).report()) {
			if(settings.stopAllRoutinesIfOneFails) {
				return Exception(moduleName, "Fail on unjam B");
			}
			failedAnywhere = true;
		}

		if(failedAnywhere) {
			return Exception(moduleName, "Fail");
		}
		
		log(LogLevel::Status, moduleName, "end");
		return Exception::None();
	}

	//----------
	Exception
	Routines::tuneCurrent(const MotionControl::MeasureRoutineSettings & settings)
	{
		// create moduleName
		char moduleName[100];
		sprintf(moduleName, "%s.tuneCurrent", this->getName());

		log(LogLevel::Status, moduleName, "begin");

		app->motionControlA->stop();
		app->motionControlB->stop();

		bool failedAnywhere = false;

		if(app->motionControlA->tuneCurrentRoutine(settings).report()) {
			if(settings.stopAllRoutinesIfOneFails) {
				return Exception(moduleName, "Fail on tuneCurrent A");
			}
			failedAnywhere = true;
		}

		if(app->motionControlB->tuneCurrentRoutine(settings).report()) {
			if(settings.stopAllRoutinesIfOneFails) {
				return Exception(moduleName, "Fail on tuneCurrent B");
			}
			failedAnywhere = true;
		}

		if(failedAnywhere) {
			return Exception(moduleName, "Fail");
		}
		
		log(LogLevel::Status, moduleName, "end");
		return Exception::None();
	}

#ifndef HOME_SWITCH_LEGACY
	//----------
	// One axis's share of Routines::calibrate() for the optical switch: fastHomeRoutine
	// replaces the measureBacklashRoutine+homeRoutine pair in a single self-calibrating pass,
	// retried up to settings.tryCount times (a failure clears fastHomeRoutine's threshold
	// cache, so each retry after the first recalibrates cold rather than repeating whatever
	// went wrong).
	//
	// A cold run's datum can sit up to ~0.2 deg off the warm one -- the calibration probing
	// perturbs the thermo-optical profile right before the precise pass (see
	// HomeSwitchTest/portalfw_port/PORTING.md item 7). So after a run that started cold
	// (opticalThresholdCached was 0 coming in), run once more immediately: the second run is
	// warm and its datum is the one to keep.
	Exception
	Routines::calibrateAxisFastHome(MotionControl * motionControl
		, const MotionControl::MeasureRoutineSettings & settings)
	{
		const bool wasCold = motionControl->opticalThresholdCached == 0;
		Exception exception = Exception::None();
		for(uint8_t i = 0; i < settings.tryCount; i++) {
			exception = motionControl->fastHomeRoutine(settings);
			if(!exception) {
				break;
			}
			log(exception);
		}
		if(exception) {
			return exception;
		}
		if(wasCold) {
			exception = motionControl->fastHomeRoutine(settings);
			if(exception) {
				log(exception);
			}
		}
		return exception;
	}
#endif

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

		bool failedAnywhere = false;

#ifndef HOME_SWITCH_LEGACY
		if(this->calibrateAxisFastHome(app->motionControlA, settings)) {
			if(settings.stopAllRoutinesIfOneFails) {
				return Exception(moduleName, "Fail on fastHome A");
			}
			failedAnywhere = true;
		}

		if(this->calibrateAxisFastHome(app->motionControlB, settings)) {
			if(settings.stopAllRoutinesIfOneFails) {
				return Exception(moduleName, "Fail on fastHome B");
			}
			failedAnywhere = true;
		}
#else
		if(app->motionControlA->measureBacklashRoutine(settings).report()) {
			if(settings.stopAllRoutinesIfOneFails) {
				return Exception(moduleName, "Fail on measure backlash A");
			}
			failedAnywhere = true;
		}
		if(app->motionControlA->homeRoutine(settings).report()) {
			if(settings.stopAllRoutinesIfOneFails) {
				return Exception(moduleName, "Fail on home A");
			}
			failedAnywhere = true;
		}

		if(app->motionControlB->measureBacklashRoutine(settings).report()) {
			if(settings.stopAllRoutinesIfOneFails) {
				return Exception(moduleName, "Fail on measure backlash B");
			}
			failedAnywhere = true;
		}
		if(app->motionControlB->homeRoutine(settings).report()) {
			if(settings.stopAllRoutinesIfOneFails) {
				return Exception(moduleName, "Fail on home B");
			}
			failedAnywhere = true;
		}
#endif

		if(failedAnywhere) {
			return Exception(moduleName, "Fail");
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

		bool failedAnywhere = false;

		if(app->motionControlA->homeRoutine(settings).report()) {
			if(settings.stopAllRoutinesIfOneFails) {
				return Exception(moduleName, "Fail on A");
			}
			failedAnywhere = true;
		}
		if(app->motionControlB->homeRoutine(settings).report()) {
			if(settings.stopAllRoutinesIfOneFails) {
				return Exception(moduleName, "Fail on B");
			}
			failedAnywhere = true;
		}

		if(failedAnywhere) {
			return Exception(moduleName, "Fail");
		}
		
		log(LogLevel::Status, moduleName, "end");
		return Exception::None();
	}

	//----------
	Exception
	Routines::measureCycle(const MotionControl::MeasureRoutineSettings & settings)
	{
		// create a moduleName
		char moduleName[100];
		sprintf(moduleName, "%s.measureCycle", this->getName());

		log(LogLevel::Status, moduleName, "begin");

		app->motionControlA->stop();
		app->motionControlB->stop();

		bool failedAnywhere = false;

		if(app->motionControlA->measureCycleRoutine(settings).report()) {
			if(settings.stopAllRoutinesIfOneFails) {
				return Exception(moduleName, "Fail on A");
			}
			failedAnywhere = true;
		}
		if(app->motionControlB->measureCycleRoutine(settings).report()) {
			if(settings.stopAllRoutinesIfOneFails) {
				return Exception(moduleName, "Fail on B");
			}
			failedAnywhere = true;
		}

		if(failedAnywhere) {
			return Exception(moduleName, "Fail");
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