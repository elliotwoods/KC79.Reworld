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

		if(app->motionControlA->unjamRoutine(settings).report()) {
			return Exception("Routines::startup : Fail on unjam A");
		}

		if(app->motionControlB->unjamRoutine(settings).report()) {
			return Exception("Routines::startup : Fail on unjam B");
		}

		if(app->motionControlA->tuneCurrentRoutine(settings).report()) {
			return Exception("Routines::startup : Fail on tuneCurrent A");
		}

		if(app->motionControlB->tuneCurrentRoutine(settings).report()) {
			return Exception("Routines::startup : Fail on tuneCurrent B");
		}

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
	Routines::walkBackAndForth(const MotionControl::MeasureRoutineSettings &settings)
	{
		// This routine rotates back and forth by 110% and reports if switchs are not seen
		log(LogLevel::Status, "Routines::walkBackAndForth : begin");

		auto routineStart = millis();
		auto routineDeadline = routineStart + (uint32_t)settings.timeout_s * 1000;

		auto & motionControlA = this->app->motionControlA;
		auto & motionControlB = this->app->motionControlB;

		auto & homeSwitchA = this->app->homeSwitchA;
		auto & homeSwitchB = this->app->homeSwitchB;

		// Instruct move
		auto moveStartA = motionControlA->getPosition();
		auto moveStartB = motionControlB->getPosition();
		auto movement = motionControlA->getMicrostepsPerPrismRotation() * 11 / 10; // 110%
		auto moveEndA = moveStartA + movement;
		auto moveEndB = moveStartB + movement;

		motionControlA->setTargetPosition(moveEndA);
		motionControlB->setTargetPosition(moveEndB);

		// Get default values (for timeout)
		MotionControl::MeasureRoutineSettings measureRoutineSettings;

		// Change the interrupt to something that looks for switches
		struct {
			bool a_forwards = false;
			bool a_backwards = false;
			bool b_forwards = false;
			bool b_backwards = false;
		} switchesSeen;

		{
			motionControlA->disableInterrupt();
			motionControlB->disableInterrupt();

			motionControlA->attachCustomInterrupt([&]() {
				switchesSeen.a_forwards |= homeSwitchA->getForwardsActive();
				switchesSeen.a_backwards |= homeSwitchA->getBackwardsActive();

				motionControlA->stepsInInterrupt++;
			});

			motionControlB->attachCustomInterrupt([&]() {
				switchesSeen.b_forwards |= homeSwitchB->getForwardsActive();
				switchesSeen.b_backwards |= homeSwitchB->getBackwardsActive();

				motionControlB->stepsInInterrupt++;
			});
		}

		auto endRoutine = [&]() {
			motionControlA->disableCustomInterrupt();
			motionControlB->disableCustomInterrupt();

			motionControlA->enableInterrupt();
			motionControlB->enableInterrupt();
		};

		// Wait for move
		log(LogLevel::Status, "Walk routine CW");
		while (motionControlA->getPosition() < motionControlA->getTargetPosition()
		 || motionControlB->getPosition() < motionControlB->getTargetPosition())
		{
			motionControlA->update();
			motionControlB->update();
			App::updateFromRoutine();
			HAL_Delay(1);

			if (millis() > routineDeadline)
			{
				endRoutine();
				return Exception::Timeout();
			}
		}

		// Instruct move back to 0
		motionControlA->setTargetPosition(0);
		motionControlB->setTargetPosition(0);

		// Wait for move
		log(LogLevel::Status, "Walk routine CCW");
		while (motionControlA->getPosition() > motionControlA->getTargetPosition() || this->app->motionControlB->getPosition() > this->app->motionControlB->getTargetPosition())
		{
			motionControlA->update();
			motionControlB->update();
			App::updateFromRoutine();
			HAL_Delay(1);

			if (millis() > routineDeadline)
			{
				endRoutine();
				return Exception::Timeout();
			}
		}

		// Announce switches seen
		{
			log(LogLevel::Status, "Switches seen:");
			switchesSeen.a_forwards
				? log(LogLevel::Status, "A Forwards :\t true")
				: log(LogLevel::Error, "A Forwards :\t false");
			switchesSeen.a_backwards
				? log(LogLevel::Status, "A backwards :\t true")
				: log(LogLevel::Error, "A backwards :\t false");
			switchesSeen.b_forwards
				? log(LogLevel::Status, "B Forwards :\t true")
				: log(LogLevel::Error, "B Forwards :\t false");
			switchesSeen.b_backwards
				? log(LogLevel::Status, "B backwards :\t true")
				: log(LogLevel::Error, "B backwards :\t false");
		}

		endRoutine();

		motionControlA->stop();
		motionControlB->stop();

		log(LogLevel::Status, "Routines::walkBackAndForth : end");

		if((switchesSeen.a_forwards || switchesSeen.a_backwards)
			&& (switchesSeen.b_forwards || switchesSeen.b_backwards))
			{
			return Exception::None();
		}
		else {
			return Exception("Missing switches");
		}
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