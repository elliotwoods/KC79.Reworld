# Porting the fast optical homing routine into PortalFW

`FastHomeRoutine.cpp` (this folder) is a reference implementation of
`MotionControl::fastHomeRoutine` written directly against the production API
(`routineMoveTo` / `routineMoveToUntilSeeSwitch` / `routineMoveToFindSwitch`,
the `switchesArmed` + `invertSwitches` ISR latch, `backlashControl`,
`homing.switchSize`, `healthStatus`). It was developed and validated on the
HomeSwitchTest bench rig (2026-07-10); the bench original is
`HomeSwitchTest/src/bench_main.cpp` (`fastHome`, command `O`).

It replaces **both** `measureBacklashRoutine` and `homeRoutine` for the
optical switch in `Routines::calibrate()` — one routine produces the home
centre, the switch size, **and** the backlash, in a single pass.

Bench results (side A rig, 189,696 µsteps/rev):

| metric | value |
|---|---|
| warm re-home (threshold cached, 2 passes) | ~11 s (single-pass ~7 s at σ 7.4) |
| cold home incl. threshold calibration + full-rev seek | ~21 s |
| home repeatability at vEdge=2000, M=32, 2 passes | σ ≈ 5 µsteps; **max 14 over 86 consecutive homes = every home <0.03°** |
| 0.01° feasibility | not consistent on this hardware: floor = per-pass noise σ≈4 + thermo-optical drift σ≈2–4 (equilibrium batches reach max 6 = 0.011°) |
| backlash | 569 ± 17 µsteps at vEdge=4000; true (0-speed extrapolated) ≈ 530; inflates ≈ 9 µsteps per 1000 µsteps/s of pass speed (sensor lag ≈ 4–5 ms) |
| acceptance | 40/40 homes from −90°…+180° starts; 4.5–6.5 s short-seek, 8.4–12.3 s wrap-seek |
| sensor lag cross-check | home datum immune to vEdge (fwd/fwd midpoint cancels lag — measured, mean shift < 30 µsteps 2k→8k) |
| threshold drift observed across 3 days | 15–25 duty counts (whole profile) |

## Why the routine is shaped this way (bench findings)

1. **The optical "flag" is a DIP in reflection** (weak return at the marker)
   between brighter background. At comparator threshold duty `T`, the sensor
   reads ACTIVE where the local *crossing duty* < `T`. Only a `T` between the
   dip floor and the background floor sees exactly one flag per rev.
2. **The whole reflection profile drifts** — measured 15–25 duty counts of
   global shift across three days (mechanical alignment / LED / temperature).
   The usable threshold band is only ~12 counts wide. **No compile-time
   threshold constant can work**; `HOMESWITCHOPTICAL_DEFAULT_THRESHOLD` is
   only a power-on placeholder. The routine measures the local background at
   run time and sets `T = background − 10`, then self-heals with bounded
   ±adjustments if the world disagrees (can't-clear → T−8; empty lap → T+6).
3. **The threshold DAC is RC-filtered (τ ≈ 100 ms).** Any measurement that
   sweeps the duty quickly reads ~20+ counts high. Every measurement that
   feeds a threshold decision must be **settled** (≥ 2 τ per probe). The
   calibration uses a 5-probe settled binary search (~1.5 s per point).
4. **Motor stall cliff**: at 0.25 A / 32 µsteps, ramping at 100 k µsteps/s²
   the motor stalls between 30–32 k µsteps/s (it cruises 36 k+ only with
   accel ≤ 20 k/s², which wins no time). Seek speed is therefore 24 k with
   100 k/s² accel. A stalled seek reads as "flag not found" (the counter
   advances, the ring doesn't).
5. **Flank dither**: the dip's flanks are shallow (40–190 µsteps per duty
   count depending on the day), so comparator noise dithers the edge over
   tens of µsteps and can latch phantom micro-flags (9–35 µsteps wide were
   observed). The ISR latch therefore needs a **debounce**: the latch fires
   on the M-th consecutive µstep-sample in the wanted state (M = 32) and the
   reported position is the first sample of that run. Both edges bias "late"
   by the same amount, so the midpoint datum stays clean.
6. **Backlash at the trailing edge** is arithmetic-identical to
   `measureBacklashRoutine`'s engage-vs-release (`backlash = engagePos −
   disengagePos`), measured on this rig at ~530 µsteps, repeatable to ±16.
7. **Datum caveat**: home = midpoint-at-threshold-T of an analog dip. The
   datum is a function of `(T, vEdge, M)`. All three are frozen constants /
   cached values; `switchSize` regression across runs is the drift canary —
   log it.

## Integration checklist

1. **Add the routine**: drop `fastHomeRoutine` into `MotionControl.cpp`, add
   the declaration + two members to `MotionControl.h`:
   ```cpp
   public:
       Exception fastHomeRoutine(const MeasureRoutineSettings&);
   protected:
       int16_t opticalThresholdCached = 0;          // 0 = not calibrated
       volatile uint16_t switchLatchDebounce = 32;  // µsteps of agreement
   ```

2. **ISR debounce patch** (`MotionControl::enableInterrupt`, the
   `attachInterrupt` lambda): replace the one-shot latch with a
   consecutive-run counter per channel. Position is reported at the first
   sample of the confirmed run:
   ```cpp
   // members: volatile uint16_t fwRun = 0, bwRun = 0;
   if(!switchesSeen.forwards.seen) {
       if(this->homeSwitch.getForwardsActive() ^ this->inInterrupt.invertSwitches) {
           if(++this->inInterrupt.fwRun >= this->switchLatchDebounce) {
               switchesSeen.forwards.seen = true;
               switchesSeen.forwards.stepCountFirstSeen =
                   this->inInterrupt.stepCount - (this->switchLatchDebounce - 1);
           }
       } else {
           this->inInterrupt.fwRun = 0;
       }
   }
   // (mirror for backwards)
   ```
   Note `stepCountFirstSeen` is a frame-local pulse count; subtracting
   (M−1) pulses shifts the replayed position (M−1) µsteps against the travel
   direction, which is exactly the first-sample-of-run position. Clamp at 0
   for the pathological case of a latch within the first M pulses of a frame.
   Keep `switchLatchDebounce = 1` for the mechanical switch build
   (`HOME_SWITCH_LEGACY`) — its contacts are clean and the legacy datum
   should not move.

3. **Rev-constant truncation** — `MOTION_STEPS_PER_PRISM_ROTATION` expands to
   `32 * 118 * 9759 / 296 / 21` in left-to-right INTEGER math = 5928, but the
   exact ratio is 5928.247 full steps: the truncated constant is **7.9
   µsteps/rev short** at 32 µsteps (bench-measured as a matching systematic
   shift after commanded full rotations). Homing is self-referencing so it
   still zeros correctly, but every position computed *from* the constant
   (multi-rev moves, `getClosestHomePosition`, degree readouts) accumulates
   the error. Fix pattern (used in BenchMotion::getMicrostepsPerPrismRotation):
   ```cpp
   const int64_t num = 32LL * 118LL * 9759LL * microstepsPerStep;
   const int64_t den = 296LL * 21LL;
   microstepsPerRotation = (Steps)((num + den / 2) / den);   // 189704 at x32
   ```

4. **Multi-pass and the drift ceiling** — repeatability is limited by slow
   thermal/mesh drift, not edge noise: at vEdge 2000 + M32 the edge loci
   repeat to ~1 µstep *within* a run, but the whole frame wanders a few
   µsteps *between* runs, and width/backlash drift visibly over minutes
   (session warm-up). Consequences, all bench-measured:
   * 2 averaged passes: successive-home σ 7.4 → **2.7 µsteps (max 6 =
     0.011°)**; 4 passes get *worse* (σ 5.0) because the 19 s run lets drift
     outpace averaging. Keep `FASTHOME_PASSES = 2`.
   * Backlash flips between two levels ~20 µsteps apart run-to-run —
     gear-mesh phase, not measurement error.
   * `switchSize` drifts with temperature at fixed T — it remains the drift
     canary, don't alarm on ±40.

5. **Comparator hysteresis** (the sensor board raises the effective
   threshold while active so it doesn't chatter off): the trailing edge
   (active→inactive, forward) trips at the *elevated*-threshold locus while
   the leading edge trips at the base locus. Both are fixed loci → the
   midpoint datum is unaffected. The backlash reading is inflated by the
   hysteresis gap mapped through the flank slope (identical to the
   mechanical switch's hysteresis inflation in production). Note
   `(lead + reenter + backlash)/2` is algebraically the same midpoint — it
   is NOT an independent estimator.

6. **int32 overflow fix** (required before raising acceleration):
   `MotionControl.cpp:826` computes `maxDeltaV = acceleration * dt_us /
   1000000` in 32-bit. At `acceleration = 100000`, any frame gap above
   ~21 ms overflows and corrupts the ramp. Cast through `int64_t` (or clamp
   `dt_us` to 10 ms):
   ```cpp
   auto maxDeltaV = (StepsPerSecond)((int64_t) this->motionProfile.acceleration
       * (int64_t) dt_us / 1000000);
   ```

7. **Wire into Routines**: in `Routines::calibrate()` replace the per-axis
   `measureBacklashRoutine(); homeRoutine();` pair with
   `fastHomeRoutine();` for optical builds. Optionally expose a
   per-axis msgpack key `"fastHome"` next to `"home"` in
   `processIncomingByKey`.

8. **Shared threshold DAC (PC15) — policy decision.** `setThreshold` is
   static: ONE RC-filtered PWM feeds BOTH axes' comparators. Each axis has
   its own dip depth and background, hence its own calibrated T. Therefore:
   * home the axes **serially** (Routines::calibrate already does), each
     setting its own T during its routine;
   * afterwards park the DAC at a compromise (e.g. `min(T_A, T_B)`), and
     treat the live sensor state as valid only near a freshly-set T.
   Anything that needs both axes' flags simultaneously (the unimplemented
   "live homing") would need per-axis DACs — out of scope.

9. **Speeds**: the seek runs at a temporarily raised `MotionProfile`
   (24 k µsteps/s, 100 k/s², min 1000); the routine restores the caller's
   profile on every exit path. The precise pass runs at `FASTHOME_EDGE_SPEED`
   (constant-speed `routineMoveToFindSwitch`), NOT at
   `settings.slowMoveSpeed`, because the datum depends on the pass speed —
   keep it a named constant shared with the bench.

10. **Failure semantics**: any failure clears `opticalThresholdCached` (next
   attempt recalibrates) and restores the default threshold. The dispatch
   layer's `tryCount` retry loop (same as `"home"`) then gives the
   calibrate-fresh attempt for free.

## Differences vs the bench implementation (`bench_main.cpp fastHome`)

The bench version additionally implements, and the port intentionally
simplifies (relying on `tryCount` retries instead):
* false-feature reject loop (re-seek past an impostor, ≤3 times),
* flank-blip retry inside the precise pass (re-arm forward in place),
* telemetry streaming (`O,...` lines).
If field units show frequent gate failures, lift those loops across —
they are straight copies.

## Numbers provenance

* Stall cliff, threshold drift, dip geometry per day, RC-lag magnitude,
  blip widths: `HomeSwitchTest/reports/` + the experiment log in the
  project README. Raw scans: repo root `home_scan_*.csv`.
* Production constants referenced: `MOTION_CLEAR_SWITCH_STEPS` (156 full
  steps ≈ 4992 µsteps ≈ FASTHOME_CLEARANCE), `debounceDistance` (32 full
  steps = 1024 µsteps = the backlash walk), `slowMoveSpeed` 2000.
