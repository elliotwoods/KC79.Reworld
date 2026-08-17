# Final injection-moulded ring gear — side A, 32:1 (2026-08-14)

Bench: STM32G070 on COM3, `home_switch_bench` rebuilt for **side A**
(`Config::A()` / `Config::MotorA()` — it was still on the B channels from the
July 16:1 work). Motor healthy: 20,000 µsteps → 38.00°, no FAULT.

## Headline

**The ring homes reliably — 50/50 successful homes from all around the circle,
with 0.005° repeatability — but only with ambient light reaching the sensor,
and only with a new threshold rule. The existing `T = background − 10`
calibration can never work on this ring in any lighting condition.**

## 1. Corrected physics

Crossing duty is the threshold the sensor output must cross, and it is
**inverse to reflectance**: *lower crossing = more reflective*. The home
feature is a **reflector** — a bright spot on a dark ring — not a dark dip.

Consequently `cross = -1, lo = 0, hi = 0` (comparator stuck LOW across 0…255)
means the surface is **darker than the sensor can measure**, i.e. crossing
above 255.

> Docs bug: the comment on `measureCrossingSettled` (`bench_main.cpp:749-754`)
> and `README.md` label stuck-LOW as "off-flag" and stuck-HIGH as
> "saturated/too strong". On this hardware stuck-LOW is the *dark* rail. Worth
> correcting — it cost real bench time this session.

## 2. The background is unmeasurable — permanently

A threshold ladder (T = 234…255, ~6τ settle per step, reading the live
comparator level) at an off-flag position:

| condition | flag crossing | background crossing |
|---|---|---|
| covered (dark) | ~250 | **> 255 — censored** |
| uncovered (ambient) | **239–245** | **> 255 — censored** |

The new moulding is dark enough in near-IR that its plain surface never
crosses, in **either** condition. There is no background number to subtract
from, so `T = background − 10` has no inputs. This is not a tuning problem;
that rule is structurally inapplicable here.

Ambient light contributes ~5–10 counts of apparent reflectance **on the flag**,
which is what makes the uncovered case workable.

## 3. Detectability band (census, settled threshold, one lap each)

The census is the trustworthy instrument: a fixed settled threshold read by the
moving comparator — exactly what homing does. (The `K` grid scan sweeps the
RC-filtered DAC and reads ~10–20 counts high; the settled `Q` probe proved
unreliable near the rails. Neither should be used for operating-point choice.)

**Uncovered:**

| T | 240 | 242 | 244 | 246 | 248 | 250 | 252 | 254 |
|---|---|---|---|---|---|---|---|---|
| segments | 1 | 1 | 1 | 1 | 1 | 2 | 1 | phantoms |
| width (µsteps) | 55 | 71 | 98 | 162 | 177 | 252 | 1295 | 1945+ |

Usable band **T ≈ 240–251, 11–12 counts** — comparable to the old ring's ~12.

**Covered:** band collapses to **T = 252–253, 2 counts**. T=255 dithers
catastrophically (22–27 census edges/lap, phantom segments up to 90° wide);
T=253 is bimodal — 14% of homes latch an outer flank, flag reads ~500 µsteps
narrow, datum error to 0.68°.

## 4. Homing results (uncovered)

| test | T | n | result |
|---|---|---|---|
| fixed-start repeatability | 248 | 12 | 12/12, datum **sd 3.12 µsteps = 0.006°**, width 188 ± 2.0 |
| adversarial, whole circle | 248 | 16 | 16/16, width 192 ± 11.8, unimodal |
| auto-calibrated (§5) | 246 | 6 | 6/6, datum **sd 2.43 µsteps = 0.0046°** |
| adversarial, whole circle | 246 | 16 | 16/16, width 149 ± 15.4, unimodal |

**50/50 homes. Repeatability 0.005° against a 0.03° target — 6× better than
needed, and better than the old ring's 0.03°.** Home time 4.1–16.4 s depending
on seek distance.

The constant ~+60 µstep bias in the fixed-start runs is uncompensated park
backlash — the bench `H` has no backlash model (`README.md:178-182`). It is a
fixed offset, not scatter, and the production `O` measures and compensates it.

*(Covered, for the record: 34/34 homes at T=252/253, but on the 2-count band
and with the T=253 bimodal failure mode above.)*

## 5. New calibration rule — validated end to end

Design in `HOME_ROUTINE_DESIGN.md`. Prototyped host-side and run against the
hardware: with **no background measurement at all** it found the band
(T_floor 240, T_shoulder 252, band 11), chose **T_op = 246**, and homed 6/6 at
0.0046°. The independent empirical optimum was 246–248 — the rule lands on it.

## 6. Verdict

**The ring, the reflector, the mechanism and the homing algorithm are all
good.** Repeatability is excellent and the failure modes seen at the band edges
are avoided by centring the threshold in the band.

Two things must change before this is production-ready:

1. **Replace the threshold rule** with the band-centred calibration
   (`HOME_ROUTINE_DESIGN.md`). Non-optional — the current rule cannot run.
2. **Resolve the ambient-light dependency.** The 11-count band exists only
   because ambient IR lands on the flag. Enclosed, it collapses to 2 counts and
   homing becomes unreliable. Either guarantee ambient reaches the sensor, or
   **raise the reflector's albedo** (the proposed dab of paint).

   Concrete spec for the paint: **get the flag's crossing to ≤ 230 measured in
   the dark.** Band width grows one-for-one with how far the flag's crossing
   drops below the 253 cap, so a flag at 230 gives a ~23-count band — robust in
   both conditions and immune to the enclosure question. Paint is the
   lower-risk fix; it removes a dependency rather than adding a constraint.

## 7. Overnight bake — 18 h, 2026-08-14 19:42 → 08-15 13:42

`monitor/overnight_newring.py`, driving the band-centred rule (recal every
30 min) + fixed-threshold `H`, starts spread around the whole circle with
alternating approach and adversarial at-home / ±179.9° wrap cycles. Room lights
off overnight. Log: `overnight.jsonl`; plot: `overnight_timeline.png`.

**3,593 / 3,593 homes succeeded. Zero failures, zero exceptions, zero
reconnects, zero below-gate recalibrations.**

| metric | result |
|---|---|
| homes | 3593 ok / 0 fail (uniform 2875, at-home 359, wrap 359 — all clean) |
| recalibrations | 33, all successful |
| band | **9–11 counts all night** (T_floor 240–242, T_shoulder fixed 252) |
| chosen T_op | **246–247** — moved by one count all night |
| flag width | mean 163, sd 17.7, unimodal (no flank-latch failure mode) |
| width per 2 h bucket | 153–166 — no thermo-optical drift problem |
| home time | mean 11.1 s, p95 17.2 s |

**The band did not collapse.** I predicted it would fall to ~2 counts once the
lights went off; it held at 9–11. Ordinary unlit-room ambient is plainly still
well above what the physical cover blocked. The enclosure risk is therefore
*real but narrower* than feared — a covered sensor measured 2 counts, an unlit
room measures 9.

The threshold rule is also far more stable than the old ring's 15–25 count/day
drift would suggest: `T_shoulder` never moved, `T_floor` moved 2 counts, `T_op`
one count, across 18 hours including a full dark/light cycle.

## 8. The one thing the bake did *not* prove

Datum scatter over the bake was **sd 75.8 µsteps = 0.14°, max 0.53°** — far
worse than the 0.005° measured in fixed-start runs, and worse than the 0.03°
target.

That is **not** a sensor result. The scatter is flat against reposition
distance (mean |shift| 60.2 / 60.7 / 61.9 / 62.2 µsteps across travel buckets
of 0–20k, 20–50k, 50–90k, 90–200k µsteps), so it is not accumulating slip — it
is a fixed per-cycle offset, i.e. **uncompensated park backlash**, exactly as
`README.md:178-182` warns for the bench `H`. Implied backlash ≈ 120 µsteps.

The sensor's own contribution is small and well measured: flag width sd
17.7 µsteps, and fixed-start datum sd 2.4–3.1 µsteps = 0.005° when gear
engagement is held constant.

So: **detection reliability is proven; datum accuracy under varied approach is
not**, because the routine used has no backlash model. The production `O`
measures backlash and does a forward-engaged park with an exact `shiftFrame` —
which removes precisely this error term. That has to be ported and re-measured
before anyone claims 0.03°.

## 9. Where this leaves us

Proven: the ring, reflector, mechanism and detection are sound — 3,593
consecutive homes with a stable, self-calibrating threshold and no drift
problem.

Outstanding, in order:
1. **Implement the band-centred calibration in firmware** and re-run the bake
   through `O` (with backlash compensation) to confirm 0.03° datum accuracy.
   This is the real remaining validation.
2. **Decide the paint.** Less urgent than I judged yesterday — an unlit room
   gives 9 counts, comfortably over the gate. But a physically enclosed sensor
   gave 2 counts, so if production units are enclosed, paint the reflector to
   ≤230 crossing measured in the dark. If they are open to room light, the
   current part is adequate as-is.
3. Confirm on a second unit — everything here is one ring on one rig.

## Artefacts

- `scans/home_scan_20260814_155218.{csv,json,png}` — operator grid scan (note:
  K-scan values read high; use the census tables above)
- `HomeSwitchTest/reports/census_T*.csv` — census laps
- Driving scripts were scratch; the algorithms are specified in
  `HOME_ROUTINE_DESIGN.md`.
