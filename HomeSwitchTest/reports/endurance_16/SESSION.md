# 16:1 module endurance / thermal session — 2026-07-20 → 22

> **Final verdict (Jul 22): the 16:1 gearing does not deliver higher
> sustained prism speed — recommend the higher gear ratio.** See
> "Day 2: sustained-duty speed characterization" below.

> **ERRATUM (Jul 22, after the 32:1 motor swap-back).** Every "backward
> long-travel dead / resonance band / backward dies when hot" finding in
> this report was measured with from-park backward jogs of exactly one
> revolution — a **degenerate instrument**: the flag lands back at the
> sensor whether the ring moved perfectly or not at all, and a correctly
> executed backward rev reads `home ≈ −rev`, which the analysis scored as
> "total stall". Direct observation on the 32:1 motor (streaming the sensor
> during a backward full rev: exactly one flag pulse at the expected
> position) proved backward motion executes cleanly on this ring. The 16:1
> backward capability is therefore **unverified, not disproven**; the
> "mechanism backward damage / tight spot" claims are retracted. What
> stands (measured with valid forward-frame instruments): the sustained
> forward-speed derating table below, the thermal duty-cycle collapse, the
> park-slip and inter-pass-slip fixes, the auto-sleep result, and the 16:1
> motor's own cold forward degradation by Jul 22 (the same ring runs 32k
> forward cleanly under the 32:1 motor, so that degradation was in the
> 16:1 motor's gearbox). The forward-only production recommendation becomes
> a *precaution pending re-measurement*, not a hard requirement.

Autonomous bench session following the 16:1 bring-up. Evidence:
`endurance.jsonl` (probe/rest/burst events), `../bake/bake_log.jsonl`
(epochs `p16-tuned`, `p16-repos`, `p16-200mA`, `p16-sleep`),
`session_timeline.png`.

## What was established

**Steps/rev = 92,252 ± 2** (k-rev homing ladder, k=3..5). The nominal
"half of 32:1" (94,852) is 2.8% wrong → implied motor-gearbox ratio
~15.562:1, not 16:1. Encoded as a measured special case in
`BenchMotion::microstepsPerPrismRotationFor`.

**Motor detection** (fast-home Phase 1b, lead-to-lead lap): classified
16:1 correctly on every attempt all session (measured revs 92,145–92,533,
far inside the ±10% window). Pre-detection seeks run at the slower
generation's cruise — the 16:1 motor stalls hard at the 32:1 24k cruise
(found live when the first post-tuning cold home failed).

**Stall envelope (cold)**: forward cliff 17–19k @100k/s² → seek 14k.
Backward: mid-band resonance stalls a 5–6k cruise dead; 4k and 7–10k clean.

**Thermal behaviour — the defining constraint of this module.** Sustained
duty at 0.25 A collapses the envelope in ~20 min: backward first (band
widens until even a 2k crawl dies), then forward (14–16k slips thousands of
µsteps/rev). Recovery: ~15 min rest at midday, but the recovery threshold
lengthened through the day (a 60-min coils-off rest no longer recovered
backward-2k from the park sector by 14:25) → the backward deficit is partly
**position/history-dependent** (bistable: dead from park at 2k, fine from
+90° minutes later; 1k crawl and full-step always work). Suspect a tight
spot / grease state near the park sector on this particular motor+mesh.

## Fixes landed (all flashed + mirrored into portalfw_port)

1. `reposSpeed` (16:1 = 4k) for every datum-critical approach move — slip
   on the backward inter-pass re-approach was biasing the datum ~300
   µsteps/pass at 14k.
2. **All in-routine moves ramped** — a hot motor slips ~780 µsteps on the
   old unramped 14,080 standing-start park legs (was a direct datum error).
3. **Driver auto-sleep** after 3 s idle (nSLEEP low, zero coil current,
   never mid-routine). Measured: no datum cost across sleep/wake (geartrain
   friction holds the rotor). Holding current was the dominant heat source.
4. Harness: 16:1 long repositions **forward-only** (long-way-around),
   backward legs ≤2k crawl; direction-aware cruise speeds; per-generation
   backlash anomaly bands; bake baseline forced cold so the banner sets
   usteps_per_rev before any home_err math.

## Endurance results

| epoch | mitigations | cycles | ok | hard fails |
|---|---|---|---|---|
| p16-tuned | none (24k-era harness speeds) | 17 | 17 | 0 (halted on recovery) |
| p16-repos | reposSpeed only | 26 | 24 | 2 (thermal collapse @~20 min) |
| p16-200mA | 200 mA current | 5 | 3 | 2 (immediate; current not the lever) |
| **p16-sleep** | **all of the above** | **113** | **112** | **1** |

With all mitigations, two consecutive 20-min bakes ran 58/58 and 54/55
from full-circle starts (golden-angle uniform + adversarial + cold
re-detections every 5th). Anomalies in those batches are reposition-slip
records — expected on a hot motor, absorbed and corrected by every home.
20-min-run / 15-min-rest is NOT a thermal equilibrium (cycle 2 ran hotter
than cycle 1); treat sustained duty as bounded.

Final cold validation: 12 warm homes sd 2.1 µsteps / span 7 (0.013°);
forced re-detection cold home clean. Reproducible +72 µstep park
undershoot in the current mechanism state (tight spot near park) — datum
unaffected (each home re-references the flag).

## Day 2: sustained-duty speed characterization (Jul 21 evening → 22)

The decisive production question: can 16:1 beat the 32:1's proven sustained
prism speed (24k on 189,704 = **45.5°/s**, held all night without thermal
issues)? Parity requires the 16:1 to sustain 11,672 µsteps/s on 92,252.

Protocol: realistic duty — forward jogs 0.1–0.5 rev with 3 s dwells (driver
auto-sleeps at 1 s idle), warm home every 10 moves measuring block slip;
degraded = two consecutive blocks |slip| > 2000. Evidence:
`endurance.jsonl` (`sustain_*` events), `current_sweep.jsonl`.

**Current is not the lever** (time-to-degradation at 10k, 39°/s):
250 mA → 16.4 min; 150 mA → 11.7 min; 100 mA → 8.3 min. More current =
longer endurance; 250 mA (the hardware max) is correct.

**Speed derating curve at 250 mA** (time to degradation, realistic duty):

| µsteps/s | prism °/s | vs 32:1 parity | result |
|---|---|---|---|
| 12,000 | 46.8 | +3% | dead in 2.3 min |
| 10,000 | 39.0 | −14% | 8–16 min |
| 8,000 | 31.2 | −31% | 8–14 min |
| 7,000 | 27.3 | −40% | 15 min |
| **6,000** | **23.4** | **−49%** | **90 min clean (round B); 61 min in round C after accumulated heat** |

**Sustainable 16:1 prism speed ≈ 23–27°/s — roughly HALF the 32:1's proven
45.5°/s.** The nominal 2× speed advantage of the gearing inverts under
thermal reality: halved gearing doubles reflected torque, the motor runs at
its limit, and the envelope collapses with heat. 16:1 wins only short cold
bursts (66–74°/s for tens of seconds from cold).

**Mechanism wear (caveat on all Day-2 numbers).** The unit degraded
progressively: Jul 21 morning cold-perfect (fwd −45 @12k, bwd clean @4k) →
Jul 21 evening sector-dependent backward → **Jul 22 morning, after a 7 h
cold soak: fwd +8,991 @12k, backward broken at all speeds, detection rev
reading 5% high**. That is permanent damage, plausibly accelerated by
hundreds of stall events during characterization (and the day-one
miswiring). A healthy unit would post better absolute numbers — but the
thermal margin problem is intrinsic to the gearing (the 17–19k cold cliff
vs the 32:1 motor's 30–32k was measured on day one, pre-damage). Homing
still converged even on the failed mechanism (homes settle to ±600 after
flushing slip — the routine's robustness result stands).

## Day 3: 32:1 motor campaign (Jul 22 → 23, same ring, motor swapped back)

User questions: (1) consistent performance at high duty? (2) better overall
than 16:1? (3) best current level / current-change strategy?

**Bring-up**: detection classified 32:1 first try (measured 189,638); cold
envelope clean and SYMMETRIC to ≥32k µsteps/s BOTH directions (fwd/bwd
ladders, non-degenerate instrument); stability sd 2.2 µsteps.

**(1) High-duty consistency: YES.** Realistic duty (0.1–0.5-rev moves, 3 s
dwells, auto-sleep active), warm home every 10 moves:
* 40-min runs at 24k/28k/30k/32k @250 mA — ALL clean, slip flat
  (median ~160/block ≈ 55 µsteps/rev = normal open-loop scatter), no growth.
* Overnight: **3 h 20 m CONTINUOUS at 24k/150 mA — 1,720 moves, zero
  degradation, T 232–233 throughout**, then a healthy probe and another
  31 min clean before the host PC (not the bench) killed the session.
* Cumulative campaign: ~8 h of high-duty operation, no thermal event of any
  kind (contrast: the 16:1 collapsed within ~20 min at every configuration).

**(2) Better than 16:1: unambiguous.**

| | 16:1 | 32:1 |
|---|---|---|
| sustained prism speed (realistic duty) | 23–27°/s | **≥60.7°/s** (32k, 40 min clean; higher untested) |
| burst (cold) | 66–74°/s briefly | ≥60.7°/s continuous — burst ceiling not reached |
| thermal endurance at duty | minutes | hours, no derating observed |
| homing repeatability (cold) | sd 2.1–3.0 | sd 2.2–3.5 |
| warm home time | ~8 s | ~11.3 s |

The 16:1's only remaining edge is warm-home duration (~3 s). For prism
speed — the reason 16:1 existed — the 32:1 sustains ≥2.2× more.

**(3) Current: run LOW, statically.** At 24k, 100/150/200/250 mA are
indistinguishable in slip (median 159–176) and homing precision
(sd 2.9–3.5) — the torque margin is that deep. Recommendation: **150 mA
fixed** (36% of the I²R heat of 250, with 50% current headroom over the
also-clean 100), plus the 1-s idle auto-sleep. Dynamic current strategies
(boost-per-move, hot-derating) buy nothing here and add failure modes; the
July heating lesson says lower steady current + sleep-when-static IS the
strategy.

## Recommendations

* **Choose the higher gear ratio for production.** The 16:1 cannot sustain
  even parity prism speed under realistic duty (2–5 s dwells); its
  sustainable ~23–27°/s is half the 32:1's proven 45.5°/s. If higher prism
  speed is the goal, it must come from elsewhere (motor choice, voltage,
  drive electronics), not from halving the gearing on this motor.
* **This specific 16:1 motor is worn out** (grossly degraded cold envelope
  by Jul 22) — replace before any further 16:1 testing; re-verify a fresh
  unit's cold cliffs before trusting the absolute numbers above.
* **Keep regardless of gearing** (all implemented + measured): driver
  auto-sleep when static (1 s idle; zero datum cost — holding current was
  the dominant heat source), all in-routine moves ramped (hot motors slip
  hundreds of µsteps on unramped starts), forward-only long travel policy,
  short backward hops at reposSpeed (never failed once, ~250+ homes).
* The homing routine itself needed no thermal concessions: T self-tracked
  231 throughout, gates never false-fired, and every hard failure was a
  stalled-mechanism consequence, automatically recovered — including on
  the fully worn mechanism.
