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
| overnight resilience bake (2026-07-10→11, 15.5 h) | **2,337/2,338 homes** from full-circle starts (uniform sweep + on-edge/in-flag/wrap/boundary adversarial, alternating approach directions, cold recal every 10th, ±2-rev detours). The 1 failure exposed the sector-bistable threshold + fragile depth gate (both fixed, see below); **1,955/1,955 clean after the fix**. Warm back-to-back repeatability at unchanged T held sd 5.2 / max 14 µsteps (<0.03°) across the whole night; T self-tracked 228–234; backlash breathed 412–796 with temperature |
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
   **And the background also varies ~8 duty counts by ring sector** (overnight
   bake, 2026-07-11): a T derived at the arbitrary starting position is
   bistable (231 vs 239 on the rig; at 239 the flag reads near its shoulders —
   ~2× width, ~2.5× backlash, ~10× worse repeatability, and within ~60 µsteps
   of the width gate). The final T must therefore be **anchored at a
   flag-referenced spot** — the routine re-derives it from a settled probe at
   `lead − CLEARANCE` (the pass arming point) on every cold run, and the depth
   gate verdict is `dip ≤ anchorBg − 6` (floor sits 10–14 counts below the
   background, false features 2–3; judging depth against T is fragile because
   T sits only ~4 counts above the floor — evening drift false-rejected the
   real flag on the bench).
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
   `processIncomingByKey`. **Run it twice when cold**: a cold run's datum
   sits up to ~114 µsteps (0.2°) off the warm datum (the calibration
   probing dwells inside the flag and perturbs the thermo-optical profile
   just before the precise pass — overnight-bake measurement, corr −0.54
   between a cold home's error and the next home's correction). The second,
   warm run (~11 s) lands on the production datum; warm homes then repeat
   to σ ≈ 7 µsteps across a whole night, drift included.

8. **Shared threshold DAC (PC15) — policy decision.** `setThreshold` is
   static: ONE RC-filtered PWM feeds BOTH axes' comparators. Each axis has
   its own dip depth and background, hence its own calibrated T. Therefore:
   * home the axes **serially** (Routines::calibrate already does), each
     setting its own T during its routine;
   * afterwards park the DAC at a compromise — **now implemented**:
     `Routines::calibrate` parks it at `min(T_A, T_B)` (default 235 if either
     axis is uncalibrated) after both axes home. Previously the success path
     left it at whichever axis homed *last*, so one axis's comparator was read
     at the other's threshold; treat the live sensor state as valid only near a
     freshly-set T regardless.
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

11. **The census (`homeSwitchCensusRoutine`, operator `:n`) is
   position-fragile** — it reports correctly from a clean position well before
   the flag but has been observed to miss a present flag when started at/near
   the home datum (home parks *off* the flag). It is the trusted instrument for
   *choosing* a threshold by hand, but do **not** build an automatic pass/fail
   gate on it until this is fixed: a per-`T_op` "exactly one segment per
   revolution" verification was written and pulled for exactly this reason. See
   `HomeSwitchTest/reports/newring/THRESHOLD_STRATEGY.md`.

12. **Threshold defence layer** — every `(T, W)` pair is screened against a
   clean operating band (`MotionControl::opticalPointPlausible`,
   `FASTHOME_T_OP_MIN/MAX`, `FASTHOME_W_MIN/MAX`) at persist, restore,
   success-cache, the seeded width gate, and the cold `T_op` clamp. This is what
   stops a smeared calibration (a high threshold with a ballooned width) from
   being persisted or trusted. Full account:
   `HomeSwitchTest/reports/newring/THRESHOLD_STRATEGY.md`.

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
  steps ≈ 4992 µsteps ≈ the 32:1 clearance), `debounceDistance` (32 full
  steps = 1024 µsteps = the 32:1 backlash walk), `slowMoveSpeed` 2000.

## The two module generations (FastHomeParams)

Bench session Jul 20 2026 (16:1 module, side-B motor + side-B sensor):

| quantity | 32:1 (original) | 16:1 (2026) | note |
|---|---|---|---|
| µsteps/rev | 189,704 (exact rational) | **92,252 measured ±2** | nominal half (94,852) is wrong by 2.8%; implied gearbox ≈15.562:1 |
| stall cliff @100k/s² | 30–32k | forward 17–19k; **backward resonance band 5–6k cold, swallows 10–14k hot** | halved gearing doubles reflected torque |
| seek cruise (fwd) | 24,000 | 14,000 | ≥20% margin; seek slip is self-correcting |
| approach/repos speed | 24,000 | **4,000** | below any resonance band; slip on the inter-pass re-approach biases the datum (~300 µsteps/pass measured at 14k) |
| vEdge / M | 2000 / 32 | 2000 / 48 | 16:1 dip is ~24 counts deep (vs 10–14), steep flanks; 3000 is past the knee |
| flag width @T_op | 1263–1821 (breathes) | ~760–842 | |
| backlash | 412–796 (overnight) | 144–476 seen | motor-gearbox dominated — does NOT scale with rev |
| stay-at-park sd / max datum err | 5.2 / 14 µsteps overnight | 3.0 / 7 µsteps (30 homes, reposSpeed fix) | 16:1 max datum error 0.027° — inside the 0.03° bar; note the bar is 2× harder per µstep at 16:1 |
| warm home time | ~11–14 s | ~8.1 s mean | |

Detection (Phase 1c) measured rev spread over 5 runs: 92,145–92,416 —
coarse but far inside the ±10% classification window. The ±2 precision
number comes from the k-rev homing ladder (jog k·rev, home, home/k).

Two additional 16:1 operating rules (bench, Jul 21):

* **Sleep the driver when static.** Holding current is the dominant heat
  source and sustained heat collapses the 16:1 stall envelope within
  ~20 min (backward first, then forward). Measured: de-energize/re-energize
  across an idle period adds **no datum error** beyond the noise floor —
  geartrain friction holds the rotor (the bench auto-sleeps nSLEEP after 3 s
  idle, never mid-routine). Production should do the same; note nSLEEP
  resets the driver's microstep indexer, so only sleep between operations.
* **No constant-speed standing starts.** A hot 16:1 motor slips hundreds of
  µsteps on an unramped start at the old 14,080 default (measured ~780 on
  the park legs — a direct datum error). Every in-routine positioning move
  must ramp (the bench now uses ramped moves at `reposSpeed` throughout;
  production's profile moves already ramp).
