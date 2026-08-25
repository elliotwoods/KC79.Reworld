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

		app->motionControlA->stop();
		app->motionControlB->stop();

		auto duration = [&]() {
			char message[100];
			sprintf(message, "Duration: %ds", (int) ((millis() - startTime) / 1000));
			log(LogLevel::Status, moduleName, message);
		};

		// Check before calibrating, and stop here if the check fails.
		//
		// Calibration is the expensive half of startup -- on an axis whose flag cannot be found
		// it spends minutes sweeping the threshold to 255 before saying so -- and every bit of
		// that cost is wasted on a module that cannot turn its prisms. The check answers that
		// question in seconds, for both axes at once.
		//
		// A failure stops the WHOLE routine, including the axis that passed. That is not
		// giving up early on a good axis: the fault pattern the observer needs is on the idle
		// LEDs, LEDs::update() does not run while a routine is blocking, and so the fastest way
		// to say WHICH axis is bad is to stop being in a routine.
		if(this->cycleCheck(settings).report()) {
			duration();
			return Exception(moduleName, "Fail on cycleCheck");
		}

		// Back to the ordinary routine blink for calibration -- the double rate belongs to the
		// check, which is over.
		App::setRoutineSignal(App::RoutineSignal::Normal);

		if(this->calibrate(settings).report()) {
			duration();
			return Exception(moduleName, "Fail on calibrate");
		}

		duration();

		// Move to zero position on both axes. Only now: calibrate() has just established what
		// zero means, and commanding a move to it before that is a move to nowhere in
		// particular -- on a module that has already failed, potentially a long one.
		app->motionControlA->setTargetPosition(0);
		app->motionControlB->setTargetPosition(0);

		log(LogLevel::Status, moduleName, "end");
		return Exception::None();
	}

	//----------
	// The fast check, on both axes at the same time.
	//
	// Simultaneously because there is no reason not to be: unlike calibration, this needs
	// nothing per-axis from the shared threshold DAC -- it runs at the power-on threshold and
	// asks about geometry, not optics. Both axes step and latch concurrently, which is what the
	// module already does whenever it moves both prisms at once.
	//
	// Both axes must pass before either is calibrated. Calibration is serial and a failure
	// abandons the whole routine, so calibrating A while B is still being checked would be
	// paying for an axis we may be about to walk away from.
	Exception
	Routines::cycleCheck(const MotionControl::MeasureRoutineSettings & settings)
	{
		char moduleName[100];
		sprintf(moduleName, "%s.cycleCheck", this->getName());

		log(LogLevel::Status, moduleName, "begin");

		app->motionControlA->stop();
		app->motionControlB->stop();

		// Double-rate blink for the duration of the check, so the short "is this module even
		// turning" phase is distinguishable from the ordinary routine blink of everything
		// else. Set here rather than in startup() so a check run on its own says the same
		// thing; App::update() puts it back to Normal when the routine chain ends.
		App::setRoutineSignal(App::RoutineSignal::Checking);

		// Deliberately not seeded from settings.timeout_s: that 240 s is the ceiling the cold
		// optical calibration needs, and this check is bounded by distance at about 27 s.
		MotionControl::CycleCheckSettings checkSettings;

		auto exceptionA = app->motionControlA->cycleCheckBegin(checkSettings);
		auto exceptionB = app->motionControlB->cycleCheckBegin(checkSettings);

		bool runningA = !exceptionA;
		bool runningB = !exceptionB;
		bool escaped = false;

		while(runningA || runningB) {
			if(runningA) {
				runningA = app->motionControlA->cycleCheckUpdate();
			}
			if(runningB) {
				runningB = app->motionControlB->cycleCheckUpdate();
			}

			HAL_Delay(1);

			// Once per tick for the module, not once per axis -- it feeds the watchdog, drains
			// the console and services RS485, and doing it twice per tick would double the
			// console's command rate inside the routine.
			if(App::updateFromRoutine()) {
				escaped = true;
				break;
			}
		}

		auto resultA = app->motionControlA->cycleCheckEnd();
		auto resultB = app->motionControlB->cycleCheckEnd();

		// Both verdicts on one line, because "which axis" is the question being asked.
		{
			char message[110];
			sprintf(message, "A: %s | B: %s"
				, MotionControl::cycleCheckVerdictName(app->motionControlA->getCycleCheckVerdict())
				, MotionControl::cycleCheckVerdictName(app->motionControlB->getCycleCheckVerdict()));
			log(LogLevel::Status, moduleName, message);
		}

		if(escaped) {
			return Exception::Escape(moduleName);
		}

		bool failedAnywhere = false;

		// report() both before deciding: `||` would short-circuit, and an axis whose check
		// never began would then silence the other axis's verdict.
		{
			const bool beginFailedA = exceptionA.report();
			const bool checkFailedA = resultA.report();
			if(beginFailedA || checkFailedA) {
				if(settings.stopAllRoutinesIfOneFails) {
					return Exception(moduleName, "Fail on A");
				}
				failedAnywhere = true;
			}
		}

		{
			const bool beginFailedB = exceptionB.report();
			const bool checkFailedB = resultB.report();
			if(beginFailedB || checkFailedB) {
				if(settings.stopAllRoutinesIfOneFails) {
					return Exception(moduleName, "Fail on B");
				}
				failedAnywhere = true;
			}
		}

		if(failedAnywhere) {
			return Exception(moduleName, "Fail");
		}

		log(LogLevel::Status, moduleName, "end");
		return Exception::None();
	}

	//----------
	// Both axes at once, and at full torque.
	//
	// Simultaneously, not one after the other, for two reasons. A module is jammed as a module:
	// running A to completion and only then starting B doubles a sweep that is already minutes
	// long, and it hides the case where freeing one axis disturbs the other. And the home-flag
	// consistency each axis reports is only comparable between them if both were measured under
	// the same supply and the same thermal load, which means at the same time.
	//
	// The shared MotorDriverSettings -- one VREF pin, one microstep pair, both axes -- is taken
	// to its full-torque, full-step operating point HERE, once, before either axis begins. If
	// each axis captured its own prior, axis B would capture axis A's override and restore the
	// wrong current at the end.
	Exception
	Routines::unjam(const MotionControl::MeasureRoutineSettings & settings)
	{
		// create moduleName
		char moduleName[100];
		sprintf(moduleName, "%s.unjam", this->getName());

		log(LogLevel::Status, moduleName, "begin");

		auto startTime = millis();

		app->motionControlA->stop();
		app->motionControlB->stop();

		auto & driverSettings = *app->motorDriverSettings;
		const auto priorCurrent = driverSettings.getCurrent();
		const auto priorMicrostep = driverSettings.getMicrostepResolution();

		// Full torque, and full steps. Microstepping trades holding torque per step for
		// smoothness, which is the wrong trade against a jam: what frees a stuck prism is the
		// largest impulse the driver can make, delivered over and over.
		driverSettings.setCurrent(MOTORDRIVERSETTINGS_MAX_CURRENT);
		driverSettings.setMicrostepResolution(MotorDriverSettings::MicrostepResolution::_1);

		{
			char message[80];
			sprintf(message, "full torque: %dmA, full steps"
				, (int) (driverSettings.getCurrent() * 1000.0f));
			log(LogLevel::Status, moduleName, message);
		}

		MotionControl::UnjamSettings unjamSettings;

		auto exceptionA = app->motionControlA->unjamBegin(settings, unjamSettings);
		auto exceptionB = app->motionControlB->unjamBegin(settings, unjamSettings);

		bool runningA = !exceptionA;
		bool runningB = !exceptionB;
		bool escaped = false;

		while(runningA || runningB) {
			if(runningA) {
				runningA = app->motionControlA->unjamUpdate();
			}
			if(runningB) {
				runningB = app->motionControlB->unjamUpdate();
			}

			// One tick for both axes. 2ms rather than the 20ms the previous routine used: the
			// home flag is only about nine full steps wide at this speed, and a 20ms tick walks
			// ten steps between looks. It is also the shortest tick the ramp survives -- see
			// UnjamSettings::acceleration.
			HAL_Delay(2);

			// Called once per tick for the whole module, not once per axis: it feeds the
			// watchdog, drains the console and services RS485, and doing that twice per tick
			// would double the console's command rate inside the routine.
			if(App::updateFromRoutine()) {
				escaped = true;
				break;
			}
		}

		auto resultA = app->motionControlA->unjamEnd();
		auto resultB = app->motionControlB->unjamEnd();

		// Restore the shared operating point before reporting, so a module left by a failed
		// sweep is still holding its normal current.
		driverSettings.setMicrostepResolution(priorMicrostep);
		driverSettings.setCurrent(priorCurrent);

		// Any axis whose sweep ran has had its datum zeroed by unjamEnd (deliberately -- a sweep
		// that drives through a jam cannot know where it finished) and this routine does NOT
		// chain a home: freeing the mechanism and re-datuming it are separate decisions, and the
		// operator may want to inspect the report first. Say so loudly, success or failure,
		// because a host that still assumes the old datum-preserving unjam would otherwise carry
		// on commanding absolute moves in an arbitrary frame. homeOK reads false in status until
		// a home succeeds.
		if(app->motionControlA->getUnjamReport().ran
			|| app->motionControlB->getUnjamReport().ran) {
			log(LogLevel::Warning, moduleName
				, "home datum LOST on swept axes (position zeroed, homeOK=false); home before absolute moves");
		}

		// Report the two axes side by side. Whether the sweep freed anything is a judgement
		// about these numbers, so they belong in the log next to each other rather than
		// scattered through it.
		{
			const auto & reportA = app->motionControlA->getUnjamReport();
			const auto & reportB = app->motionControlB->getUnjamReport();
			char message[120];
			sprintf(message, "A: %d sightings, gap %d..%d, worst %d | B: %d sightings, gap %d..%d, worst %d"
				, (int) reportA.sightings
				, (int) reportA.shortestGap
				, (int) reportA.longestGap
				, (int) reportA.worstDeviation
				, (int) reportB.sightings
				, (int) reportB.shortestGap
				, (int) reportB.longestGap
				, (int) reportB.worstDeviation);
			log(LogLevel::Status, moduleName, message);
		}

		{
			char message[100];
			sprintf(message, "Duration: %ds", (int) ((millis() - startTime) / 1000));
			log(LogLevel::Status, moduleName, message);
		}

		if(escaped) {
			return Exception::Escape(moduleName);
		}

		bool failedAnywhere = false;

		// report() both, then decide. `||` would short-circuit, and an axis whose sweep never
		// began would then silence the OTHER axis's verdict -- which is the one result an
		// operator staring at a jammed module actually needs.
		{
			const bool beginFailedA = exceptionA.report();
			const bool sweepFailedA = resultA.report();
			if(beginFailedA || sweepFailedA) {
				if(settings.stopAllRoutinesIfOneFails) {
					return Exception(moduleName, "Fail on unjam A");
				}
				failedAnywhere = true;
			}
		}

		{
			const bool beginFailedB = exceptionB.report();
			const bool sweepFailedB = resultB.report();
			if(beginFailedB || sweepFailedB) {
				if(settings.stopAllRoutinesIfOneFails) {
					return Exception(moduleName, "Fail on unjam B");
				}
				failedAnywhere = true;
			}
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
	// A CALIBRATED run's datum can sit up to ~0.2 deg off a warm one -- the calibration probing
	// perturbs the thermo-optical profile right before the precise pass (see
	// HomeSwitchTest/portalfw_port/PORTING.md item 7). So after a run that had to calibrate,
	// run once more immediately: the second run is warm and its datum is the one to keep.
	//
	// A run that adopted the compile-time default does NOT need this. The perturbation being
	// corrected for is the probing itself -- settling the shared threshold DAC through a dozen
	// steps while parked on the flag -- and a seeded run does none of it: it sets one threshold
	// and homes. Re-running it would cost a second home per axis to correct an offset that was
	// never introduced. `opticalDefaultRejected` is how the axis says which of the two it did.
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

			// An escape aborts the whole axis, not just the attempt in flight. Without this
			// the operator would have to send escape once per remaining retry (and once more
			// for the warm re-run below) before the axis actually stopped.
			if(App::getShouldEscapeFromRoutine()) {
				return exception;
			}
		}
		if(exception) {
			const float previousCurrent = this->app->motorDriverSettings->getCurrent();
			// Extra current only addresses lost motor position. A feature that is absent,
			// speed-dependent, optically weak, or internally inconsistent cannot be repaired by
			// driving both axes harder; doing so merely repeats the same scan and can persist an
			// unnecessary module-wide 250 mA setting.
			if(motionControl->getLastFastHomeFailure() != MotionControl::FastHomeFailure::Motion
				|| !this->app->getFullCurrentHomeRecovery()
				|| previousCurrent >= MOTORDRIVERSETTINGS_MAX_CURRENT) {
				log(LogLevel::Warning, motionControl->getName()
					, "home recovery current skipped: failure is not motion-related");
				return exception;
			}

			log(LogLevel::Warning, motionControl->getName()
				, "home failed at persisted current; retrying once at 250 mA");
			this->app->motorDriverSettings->setCurrent(MOTORDRIVERSETTINGS_MAX_CURRENT);
			Exception boosted = motionControl->fastHomeRoutine(settings);
			if(boosted) {
				log(boosted);
				this->app->motorDriverSettings->setCurrent(previousCurrent);
				log(LogLevel::Error, motionControl->getName()
					, "home failed at both persisted and boosted current; retained previous setting");
				return boosted;
			}

			if(!this->app->persistOperatingSettings(250, true)) {
				this->app->motorDriverSettings->setCurrent(previousCurrent);
				return Exception(motionControl->getName()
					, "250 mA recovery succeeded but settings persistence failed");
			}
			log(LogLevel::Status, motionControl->getName()
				, "250 mA recovery succeeded; promoted module current persistently");
			exception = Exception::None();
		}
		const bool didCalibrate = motionControl->opticalDefaultRejected;
		if(wasCold && didCalibrate && !App::getShouldEscapeFromRoutine()) {
			exception = motionControl->fastHomeRoutine(settings);
			if(exception) {
				log(exception);
			}
		}
		if(!exception && !this->app->persistOpticalCalibration(motionControl)) {
			return Exception(motionControl->getName()
				, "home succeeded but optical calibration persistence failed");
		}
		return exception;
	}
#endif

#ifdef HOME_SWITCH_LEGACY
	// Mechanical (PCB v4) only. On the optical build fastHomeRoutine produces the datum, the
	// flag width and the backlash in one pass, so this is dead weight in an application image
	// that is already 98% full.
	//----------
	// Normal home first uses the module-wide persisted current. A single successful retry at the
	// hardware limit promotes that shared current durably; a failed retry restores the prior value.
	Exception
	Routines::homeAxisWithRecovery(MotionControl * motionControl
		, const MotionControl::MeasureRoutineSettings & settings)
	{
		Exception original = motionControl->homeRoutine(settings);
		if(!original) return original;
		const float previousCurrent = this->app->motorDriverSettings->getCurrent();
		if(!this->app->getFullCurrentHomeRecovery()
			|| previousCurrent >= MOTORDRIVERSETTINGS_MAX_CURRENT) return original;

		log(original);
		log(LogLevel::Warning, motionControl->getName()
			, "home failed at persisted current; retrying once at 250 mA");
		this->app->motorDriverSettings->setCurrent(MOTORDRIVERSETTINGS_MAX_CURRENT);
		Exception boosted = motionControl->homeRoutine(settings);
		if(boosted) {
			log(boosted);
			this->app->motorDriverSettings->setCurrent(previousCurrent);
			log(LogLevel::Error, motionControl->getName()
				, "home failed at both persisted and boosted current; retained previous setting");
			return boosted;
		}
		if(!this->app->persistOperatingSettings(250, true)) {
			this->app->motorDriverSettings->setCurrent(previousCurrent);
			return Exception(motionControl->getName()
				, "250 mA home recovery succeeded but settings persistence failed");
		}
		log(LogLevel::Status, motionControl->getName()
			, "250 mA home recovery succeeded; promoted module current persistently");
		return Exception::None();
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

		// An escape during axis A stops the whole calibrate, rather than immediately starting
		// axis B (which is the same "escape only cancels the attempt in flight" trap as the
		// retry loop in calibrateAxisFastHome).
		if(App::getShouldEscapeFromRoutine()) {
			return Exception(moduleName, "Escape");
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
		if(this->homeAxisWithRecovery(app->motionControlA, settings).report()) {
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
		if(this->homeAxisWithRecovery(app->motionControlB, settings).report()) {
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
	// On an optical board, homing IS calibration: fastHomeRoutine produces the datum, the flag
	// width and the backlash in one pass, so there is nothing for a separate "home" to do that
	// calibrate() does not already do better. This used to call the legacy MECHANICAL
	// homeRoutine on a v6 board, which is what the 'h' key and the "home" command on the wire
	// both reached.
	Exception
	Routines::home(const MotionControl::MeasureRoutineSettings & settings)
	{
#ifndef HOME_SWITCH_LEGACY
		return this->calibrate(settings);
#else
		// create a moduleName
		char moduleName[100];
		sprintf(moduleName, "%s.home", this->getName());

		log(LogLevel::Status, moduleName, "begin");

		app->motionControlA->stop();
		app->motionControlB->stop();

		bool failedAnywhere = false;

		if(this->homeAxisWithRecovery(app->motionControlA, settings).report()) {
			if(settings.stopAllRoutinesIfOneFails) {
				return Exception(moduleName, "Fail on A");
			}
			failedAnywhere = true;
		}
		if(this->homeAxisWithRecovery(app->motionControlB, settings).report()) {
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
#endif
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
