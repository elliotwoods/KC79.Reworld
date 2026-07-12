// fastHomeRoutine - PortalFW port of the bench-proven fast optical homing
// =======================================================================
//
// One routine that replaces BOTH measureBacklashRoutine + homeRoutine for the
// optical home switch (Routines::calibrate currently runs the two separately;
// this does the complete job in one pass, self-calibrating the comparator
// threshold). Written against the real MotionControl API (MotionControl.h) -
// reference implementation, developed and validated on the HomeSwitchTest
// bench rig 2026-07-10. See PORTING.md for the integration checklist and the
// measured numbers behind every constant.
//
// What it produces (identical semantics to the mechanical routine pair):
//   * home centre       -> this->position -= home (home reads as 0)
//   * homing.switchSize -> trailing - leading edge
//   * backlashControl.systemBacklash (+ positionWithinBacklash = 0)
//   * healthStatus.homeOK / backlashOK / switchesOK
//
// Algorithm (bench-validated):
//   0 CALIBRATE  measure the LOCAL BACKGROUND crossing at two spots 3000
//                microsteps apart (at most one can sit on the dip) and set
//                T = max(bg1,bg2) - 10. The optical "flag" is a DIP in
//                reflection: background reads INACTIVE, only the dip ACTIVE.
//                The whole profile drifts day-to-day (measured 15-25 duty
//                counts across three days) so NO fixed threshold works.
//                Cached across runs; every failure clears the cache.
//   1 SEEK       forward up to 1.25 rev with the (debounced) switch latch
//                armed, at a RAISED motion profile. Self-heals a drifted
//                threshold: cannot-clear -> T down, lap-empty -> T up.
//   1b ANCHOR    (cold runs) re-derive T from a settled background probe at
//                lead - CLEARANCE - the SAME physical spot every run. The
//                background varies ~8 duty counts by ring sector, so a T
//                calibrated at the arbitrary phase-0 position is bistable
//                (bench: 231 vs 239; at 239 the flag reads near its
//                shoulders: ~2x width, ~2.5x backlash, 10x worse
//                repeatability).
//   2 GATES      depth (settled crossing at centre estimate must sit >= 6
//                counts below the ANCHOR background - the dip floor is 10-14
//                below it, false features 2-3; judging against T is fragile
//                since T sits only ~4 above the floor), shoulder (a
//                clearance below the lead must read inactive), width
//                (plausible flag width).
//   3 PRECISE    ONE forward pass latches leading + trailing edges in the
//                same engaged frame -> midpoint needs no backlash model.
//   4 BACKLASH   walk 32 fullsteps past the trailing edge (no reversal),
//                verify disengaged, reverse, latch re-entry:
//                backlash = trail - reenter  (same arithmetic as
//                measureBacklashRoutine's engage-vs-release).
//   5 APPLY      position -= home; park via a forward-engaged approach.
//
// CALLER POLICY (Routines::calibrate): after a COLD run (threshold was not
// cached), immediately run the routine ONCE MORE - the second run is warm
// (~11 s) and its datum is the one to keep. Bench-measured over an overnight
// bake: cold-run datums sit up to ~114 usteps (0.2 deg) off the warm datum
// (the calibration probing dwells inside the flag and perturbs the
// thermo-optical profile right before the precise pass); warm homes at the
// same threshold repeat to sd ~7 usteps across a whole night.
//
// ---------------------------------------------------------------------------
// ADD TO MotionControl.h (members):
//
//   public:
//     Exception fastHomeRoutine(const MeasureRoutineSettings&);
//   protected:
//     int16_t opticalThresholdCached = 0;   // 0 = not calibrated
//     // Debounced ISR latch (see PORTING.md "ISR debounce" patch):
//     volatile uint16_t switchLatchDebounce = 32;   // microsteps of agreement
//
// TUNABLES (bench-frozen values; see PORTING.md for provenance):
// ---------------------------------------------------------------------------
#define FASTHOME_BG_MARGIN        10     // T = anchor background crossing - this
#define FASTHOME_DEPTH_BELOW_BG   6      // dip probe <= anchor background - this
#define FASTHOME_CAL_BRACKET_LO   140    // settled-probe bracket [lo..255]
#define FASTHOME_CAL_ITERS        5      // binary probes (resolution ~4 duty)
#define FASTHOME_CAL_SETTLE_MS    220    // per probe; threshold RC tau ~100 ms
#define FASTHOME_SETTLE_MS        300    // after a large DAC step
#define FASTHOME_T_DOWN           8      // cannot-clear adjustment
#define FASTHOME_T_UP             6      // empty-lap adjustment
#define FASTHOME_MAX_T_ADJUST     4
#define FASTHOME_CLEARANCE        5000   // microsteps; > backlash, room to arm
#define FASTHOME_TAKEUP           2000   // overshoot-return, forward-engaged
#define FASTHOME_WIDTH_MIN        700    // plausible dip width at T
#define FASTHOME_WIDTH_MAX        4200
#define FASTHOME_HALF_WIDTH       900    // centre estimate = lead + this
#define FASTHOME_BACKLASH_MAX     3000
#define FASTHOME_SEEK_SPEED       24000  // usteps/s   } stall cliff measured at
#define FASTHOME_SEEK_ACCEL       100000 // usteps/s^2 } 30-32k @ 100k/s^2
#define FASTHOME_EDGE_SPEED       2000   // usteps/s (precise pass; datum depends
                                         // on it - keep fixed. At 2000+M32 the
                                         // edge loci repeat to ~1 ustep in-run)
#define FASTHOME_PASSES           2      // precise passes averaged per home.
                                         // 2 is the measured sweet spot: sd of
                                         // successive homes 7.4 -> 2.7 usteps;
                                         // MORE passes get WORSE (4 passes ->
                                         // 5.0) because the longer run lets
                                         // thermal/mesh drift outpace averaging.

namespace Modules {

// --- helper: settled crossing probe at the current position ---------------
// Binary-search the comparator flip over [lo..255] with a real RC settle per
// probe (the threshold DAC is RC-filtered, tau ~100 ms - fast sweeps read
// ~20+ duty high and MUST NOT be used to derive thresholds). Returns the
// crossing duty; -1 = stuck (railLo tells which rail); -2 = aborted.
// Leaves the DAC at the last probe - caller restores it.
static int settledCrossingProbe(HomeSwitch& homeSwitch, bool& railLo,
	uint32_t timeoutTime)
{
	auto settle = [&](uint8_t duty) -> bool {
		HomeSwitch::setThreshold(duty);
		uint32_t until = millis() + FASTHOME_CAL_SETTLE_MS;
		while(millis() < until) {
			if(App::updateFromRoutine()) return false;   // abort
			if(millis() > timeoutTime)   return false;
			HAL_Delay(1);
		}
		return true;
	};

	railLo = false;
	if(!settle(FASTHOME_CAL_BRACKET_LO)) return -2;
	const bool atLo = homeSwitch.getForwardsActive();
	if(!settle(255)) return -2;
	const bool atHi = homeSwitch.getForwardsActive();
	if(atLo == atHi) {
		railLo = atLo;
		return -1;                        // stuck: dark (inactive) or deep (active)
	}
	int lo = FASTHOME_CAL_BRACKET_LO, hi = 255;
	for(int i = 0; i < FASTHOME_CAL_ITERS; i++) {
		int mid = (lo + hi) / 2;
		if(!settle((uint8_t) mid)) return -2;
		if(homeSwitch.getForwardsActive() == atLo) lo = mid; else hi = mid;
	}
	return (lo + hi) / 2;
}

static int normCrossing(int c, bool railLo)
{
	if(c >= 0) return c;
	return railLo ? FASTHOME_CAL_BRACKET_LO : 255;   // deep / dark
}

//----------
Exception
MotionControl::fastHomeRoutine(const MeasureRoutineSettings& settings)
{
	const char * moduleName = this->getName();
	this->stop();
	if(!this->timer.hardwareTimer) {
		return Exception(moduleName, "No hardware timer");
	}
	const auto microstepsPerStep = this->motorDriverSettings.getMicrostepsPerStep();
	const Steps rev = this->getMicrostepsPerPrismRotation();
	const Steps lapBudget = rev + rev / 4;
	const uint32_t timeoutTime = millis() + (uint32_t) settings.timeout_s * 1000U;
	const StepsPerSecond vEdge = FASTHOME_EDGE_SPEED;

	auto endRoutine = [this]() {
		this->stop();
		this->switchesArmed = false;
		this->inInterrupt.invertSwitches = false;
	};
	auto fail = [&](const char * msg) -> Exception {
		this->opticalThresholdCached = 0;                     // force recalibration
		HomeSwitch::setThreshold(HOMESWITCHOPTICAL_DEFAULT_THRESHOLD);
		endRoutine();
		return Exception(moduleName, msg);
	};

	// Raised profile for the seek + repositioning moves; restored on exit.
	const MotionProfile normalProfile = this->getMotionProfile();
	MotionProfile seekProfile;
	seekProfile.maximumSpeed = FASTHOME_SEEK_SPEED;
	seekProfile.acceleration = FASTHOME_SEEK_ACCEL;     // NB int32 overflow fix
	seekProfile.minimumSpeed = 1000;                    // in calculateMotionState
	this->setMotionProfile(seekProfile);                // - see PORTING.md

	// ---- Phase 0 : threshold -------------------------------------------------
	int T = this->opticalThresholdCached;
	const bool warm = (T > 0);
	if(!warm) {
		bool rail1, rail2;
		int c1 = settledCrossingProbe(this->homeSwitch, rail1, timeoutTime);
		if(c1 == -2) { this->setMotionProfile(normalProfile); return fail("abort"); }
		auto r = this->routineMoveTo(this->getPosition() + 3000, timeoutTime);
		if(r.exception) { this->setMotionProfile(normalProfile); return fail("abort"); }
		int c2 = settledCrossingProbe(this->homeSwitch, rail2, timeoutTime);
		if(c2 == -2) { this->setMotionProfile(normalProfile); return fail("abort"); }
		const int b1 = normCrossing(c1, rail1), b2 = normCrossing(c2, rail2);
		T = (b1 > b2 ? b1 : b2) - FASTHOME_BG_MARGIN;
		if(T < 16)  T = 16;
		if(T > 250) T = 250;
	}
	HomeSwitch::setThreshold((uint8_t) T);
	HAL_Delay(FASTHOME_SETTLE_MS);

	// ---- Phase 1 : seek a coarse leading edge --------------------------------
	this->switchesArmed = true;
	Steps coarseLead = 0;
	int tAdjusts = 0;
	while(true) {
		if(millis() > timeoutTime) { this->setMotionProfile(normalProfile); return fail("timeout"); }
		if(this->homeSwitch.getForwardsActive()) {
			// Active at rest: on the dip, or T above the local background.
			// Exit backward, BOUNDED (an unbounded clear at a too-high
			// threshold never terminates - the whole rev reads active).
			this->inInterrupt.invertSwitches = true;    // latch the DROP
			this->updateStepsAndSwitches();
			auto r = this->routineMoveToFindSwitch(false, settings.slowMoveSpeed * 4,
				SwitchesMask{ true, false }, /* bounded: */ millis() + 4000);
			this->inInterrupt.invertSwitches = false;
			if(!r.exception && r.frameSwitchEvents.forwards.seen) {
				coarseLead = r.frameSwitchEvents.forwards.positionSeen;
				break;
			}
			if(++tAdjusts > FASTHOME_MAX_T_ADJUST) {
				this->setMotionProfile(normalProfile);
				return fail("no inactive background");
			}
			T -= FASTHOME_T_DOWN;
			if(T < 16) { this->setMotionProfile(normalProfile); return fail("threshold floor"); }
			HomeSwitch::setThreshold((uint8_t) T);
			HAL_Delay(FASTHOME_SETTLE_MS);
			continue;
		}
		// Inactive at rest: raised-profile move up to 1.25 rev, early-out on
		// the (debounced) forwards latch.
		auto r = this->routineMoveToUntilSeeSwitch(this->getPosition() + lapBudget,
			SwitchesMask{ true, false }, timeoutTime);
		if(!r.exception && r.frameSwitchEvents.forwards.seen) {
			coarseLead = r.frameSwitchEvents.forwards.positionSeen;
			break;
		}
		if(millis() > timeoutTime) { this->setMotionProfile(normalProfile); return fail("timeout"); }
		if(++tAdjusts > FASTHOME_MAX_T_ADJUST) {
			this->setMotionProfile(normalProfile);
			return fail("flag not found");
		}
		T += FASTHOME_T_UP;                     // empty lap: T below the dip floor
		if(T > 250) { this->setMotionProfile(normalProfile); return fail("threshold ceiling"); }
		HomeSwitch::setThreshold((uint8_t) T);
		HAL_Delay(FASTHOME_SETTLE_MS);
	}

	// ---- Phase 2 : depth probe (cold runs; measure only) ----------------------
	// The probe sits ~200 usteps past the true centre (seek lag shifts
	// coarseLead forward) so it reads a shallow flank; the verdict is made
	// against the anchor background below, not against T.
	int coldDepth = -1;
	if(!warm) {
		auto r = this->routineMoveTo(coarseLead + FASTHOME_HALF_WIDTH, timeoutTime);
		if(r.exception) { this->setMotionProfile(normalProfile); return fail("abort"); }
		bool rail;
		int cc = settledCrossingProbe(this->homeSwitch, rail, timeoutTime);
		if(cc == -2) { this->setMotionProfile(normalProfile); return fail("abort"); }
		coldDepth = normCrossing(cc, rail);
		HomeSwitch::setThreshold((uint8_t) T);
		HAL_Delay(FASTHOME_SETTLE_MS);
	}

	// ---- Phase 3 : precise forward two-edge pass ------------------------------
	// Overshoot-return: approach the arming point from below so the pass runs
	// forward-engaged (backlash taken up before the first edge).
	{
		auto r = this->routineMoveTo(coarseLead - FASTHOME_CLEARANCE - FASTHOME_TAKEUP, timeoutTime);
		if(r.exception) { this->setMotionProfile(normalProfile); return fail(r.exception.message); }
		r = this->routineMoveTo(coarseLead - FASTHOME_CLEARANCE, timeoutTime);
		if(r.exception) { this->setMotionProfile(normalProfile); return fail(r.exception.message); }
	}
	// ---- Phase 1b : threshold anchor + depth verdict (cold runs) ---------------
	if(!warm) {
		bool rail;
		int ac = settledCrossingProbe(this->homeSwitch, rail, timeoutTime);
		if(ac == -2) { this->setMotionProfile(normalProfile); return fail("abort"); }
		const int anchorBg = normCrossing(ac, rail);
		T = anchorBg - FASTHOME_BG_MARGIN;
		if(T < 16)  T = 16;
		if(T > 250) T = 250;
		if(coldDepth >= 0 && coldDepth > anchorBg - FASTHOME_DEPTH_BELOW_BG) {
			this->setMotionProfile(normalProfile);
			return fail("depth gate: not the home dip");   // (bench: reject+reseek)
		}
		HomeSwitch::setThreshold((uint8_t) T);
		HAL_Delay(FASTHOME_SETTLE_MS);
	}
	if(this->homeSwitch.getForwardsActive()) {
		this->setMotionProfile(normalProfile);
		return fail("shoulder gate: active below lead");
	}
	// FASTHOME_PASSES forward two-edge passes, midpoints averaged. Each pass:
	// latch the leading edge (normal latch), then the trailing edge (inverted
	// latch = becoming INACTIVE), both moving forward in the same engaged frame.
	Steps lead = 0, trail = 0;
	Steps midSum = 0, widthSum = 0;
	for(int pass = 0; pass < FASTHOME_PASSES; pass++) {
		if(pass > 0) {
			// Re-approach the arming point forward-engaged for the next pass.
			auto r = this->routineMoveTo(coarseLead - FASTHOME_CLEARANCE - FASTHOME_TAKEUP, timeoutTime);
			if(!r.exception) r = this->routineMoveTo(coarseLead - FASTHOME_CLEARANCE, timeoutTime);
			if(r.exception) { this->setMotionProfile(normalProfile); return fail(r.exception.message); }
			if(this->homeSwitch.getForwardsActive()) {
				this->setMotionProfile(normalProfile);
				return fail("active at pass arming point");
			}
		}
		auto r = this->routineMoveToFindSwitch(true, vEdge,
			SwitchesMask{ true, false }, timeoutTime);
		if(r.exception || !r.frameSwitchEvents.forwards.seen) {
			this->setMotionProfile(normalProfile); return fail("leading edge");
		}
		lead = r.frameSwitchEvents.forwards.positionSeen;

		this->inInterrupt.invertSwitches = true;     // latch becoming INACTIVE
		this->updateStepsAndSwitches();
		r = this->routineMoveToFindSwitch(true, vEdge,
			SwitchesMask{ true, false }, timeoutTime);
		this->inInterrupt.invertSwitches = false;
		if(r.exception || !r.frameSwitchEvents.forwards.seen) {
			this->setMotionProfile(normalProfile); return fail("trailing edge");
		}
		trail = r.frameSwitchEvents.forwards.positionSeen;

		if(trail - lead < FASTHOME_WIDTH_MIN || trail - lead > FASTHOME_WIDTH_MAX) {
			this->setMotionProfile(normalProfile);
			return fail("width gate");                // (bench: blip-retry+reseek)
		}
		midSum += (lead + trail) / 2;
		widthSum += trail - lead;
	}
	const Steps width = (Steps)(widthSum / FASTHOME_PASSES);
	const Steps home = (Steps)(midSum / FASTHOME_PASSES);

	// ---- Phase 4 : backlash at the trailing edge ------------------------------
	this->backlashControl.systemBacklash = 0;         // no live comp during measure
	this->backlashControl.positionWithinBacklash = 0;
	{
		auto r = this->routineMoveTo(trail + settings.debounceDistance * microstepsPerStep,
			timeoutTime);
		if(r.exception) { this->setMotionProfile(normalProfile); return fail(r.exception.message); }
	}
	if(this->homeSwitch.getForwardsActive()) {
		this->setMotionProfile(normalProfile);
		return fail("no disengage after trailing edge");
	}
	Steps reenter;
	{
		// Reverse; latch the sensor re-entering the dip (normal, non-inverted
		// latch; the optical backwards signal is the same pin).
		auto r = this->routineMoveToFindSwitch(false, vEdge,
			SwitchesMask{ false, true }, timeoutTime);
		if(r.exception || !r.frameSwitchEvents.backwards.seen) {
			this->setMotionProfile(normalProfile); return fail("backlash re-entry");
		}
		reenter = r.frameSwitchEvents.backwards.positionSeen;
	}
	Steps backlash = trail - reenter;
	if(backlash < -32 || backlash > FASTHOME_BACKLASH_MAX) {
		this->setMotionProfile(normalProfile);
		return fail("backlash out of range");
	}
	if(backlash < 0) backlash = 0;

	// ---- Phase 5 : apply -------------------------------------------------------
	{	// park via a forward-engaged approach (same frame as the edge pass)
		auto r = this->routineMoveTo(home - FASTHOME_CLEARANCE - FASTHOME_TAKEUP, timeoutTime);
		if(!r.exception) r = this->routineMoveTo(home - FASTHOME_CLEARANCE, timeoutTime);
		if(!r.exception) r = this->routineMoveTo(home, timeoutTime);
		if(r.exception) { this->setMotionProfile(normalProfile); return fail(r.exception.message); }
	}

	this->backlashControl.systemBacklash = backlash;
	this->backlashControl.positionWithinBacklash = 0;
	this->homing.switchSize = width;
	this->position -= home;
	this->targetPosition = 0;
	this->opticalThresholdCached = T;                 // warm next time
	this->healthStatus.homeOK = true;
	this->healthStatus.backlashOK = true;
	this->healthStatus.switchesOK = true;

	this->setMotionProfile(normalProfile);
	endRoutine();
	return Exception::None();
}

}
