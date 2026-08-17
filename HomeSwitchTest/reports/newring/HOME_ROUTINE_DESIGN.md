# Home routine for the injection-moulded ring — band-centred calibration

Replacement for the threshold-calibration phase of `fastHome` (`O`,
`bench_main.cpp:1195`; port `portalfw_port/FastHomeRoutine.cpp`). The seek,
precise two-edge pass, backlash and park phases are unchanged.

## Why the existing rule cannot work

`T = background − 10` (`kFastBgMargin`, `bench_main.cpp:166`) assumes:

1. the background is **measurable**, and
2. the home feature sits **≥10 counts** on the far side of it.

On this ring the background never crosses at any threshold 0…255, covered or
uncovered (§2 of `SESSION.md`). There is no background value, so the rule has
no inputs — and the depth gate `kFastDepthBelowBg = 6` has nothing to judge
against either. The feature is also a **reflector** (low crossing), the inverse
polarity to what the constants assume.

What *is* measurable is the range of thresholds over which the flag is cleanly
detected. That **detectability band** is a contrast-relative quantity, needs no
background, and self-tracks the ambient pedestal. Centre the operating
threshold in it.

## Algorithm

### Phase 0 — background guard (~2 s, static)

Three settled crossing probes spread 120° apart. The flag is <0.2% of the
circle so at most one probe can land on it; take the **maximum** (least
reflective) as the background estimate.

```
if all three censored (no crossing):   T_cap = T_CAP_DARK      // 253
else:                                  T_cap = bg - 3
```

`T_CAP_DARK = 253` sits two counts below the ceiling, outside the 254–255
dither zone measured this session. Keeping the `bg` branch means the routine
stays correct if a future ring or brighter ambient brings the background back
into range.

### Phase 1 — flag acquisition

Ramped seek at `T = T_cap`. Background is inactive by construction, so any
ACTIVE span is the flag. Require **exactly one** span per revolution; more than
one → fail `"multiple features"`.

### Phase 2 — band measurement (this replaces the old calibrate phase)

Step `T` down from `T_cap` in 2-count steps. At each step re-measure the flag
width with a **local** re-scan — jog from `lead − clearance` to
`trail + clearance` at `edgeSpeed`, latching both edges. No full lap; ~1 s per
step, ~6 steps.

```
W_med       = median width across steps where the flag was found
T_shoulder  = lowest T whose width > SHOULDER_FACTOR * W_med   // 3.0
T_floor     = lowest T still giving width >= W_MIN             // 40 usteps
band        = (T_shoulder - 1) - T_floor
```

**Gate:** `band >= BAND_MIN` (6 counts), else fail
`"insufficient optical contrast"`. This is the meaningful signal-quality check
and replaces the absolute depth gate.

### Phase 3 — operating point

```
T_op = clamp(T_floor + round(F * band), T_floor + 2, T_shoulder - 2)   // F = 0.55
```

Adopt `T_op` as the live threshold and cache it with the measured width
`W_cal` at that threshold.

### Phase 4 — unchanged

Precise two-edge pass (2 averaged, 3rd tie-breaks), backlash measurement,
forward-engaged park, `shiftFrame(home)`.

## Constants that must change

The 32:1 parameter set was tuned for a ~1900-µstep flag. At the operating
threshold this flag is **130–190 µsteps** — an order of magnitude narrower —
so every absolute geometry constant is wrong. Make them **relative to `W_cal`**
so they auto-scale and never need retuning for another ring:

| constant | now | change to |
|---|---|---|
| `widthMin` / `widthMax` (700 / 4200) | absolute | `W_cal * 0.65` / `W_cal * 1.35` |
| `halfWidthGuess` (900) | absolute | `W_cal / 2` |
| `debounceM` (32) | absolute — 20% of a 150-µstep flag | `clamp(W_cal / 8, 8, 32)` → ≈16 here |
| `kFastWidthServoBand` (45 µsteps) | absolute — was ~1.2 counts on a 1900 flag, is 30% on a 150 flag | `W_cal * 0.15` |
| `kFastBgMargin` (10) | — | **deleted**; superseded by Phase 2/3 |
| `kFastDepthBelowBg` (6) | — | **deleted**; superseded by the band gate |

New: `T_CAP_DARK = 253`, `BAND_MIN = 6`, `W_MIN = 40`,
`SHOULDER_FACTOR = 3.0`, `F = 0.55`.

## Recalibration policy

The band moves with ambient light — measured 240–251 uncovered versus 252–253
covered. So the band scan must re-run:

- on every cold start (as now), **and**
- whenever the precise pass's width drifts >25% from `W_cal`
  (the old ±45-µstep servo band is far too coarse at this flag size).

## Validation

Prototyped host-side against the hardware, using no background measurement:

```
T=240: 1 seg w=55     T=248: 1 seg w=177
T=242: 1 seg w=71     T=250: 2 seg w=252
T=244: 1 seg w=98     T=252: 1 seg w=1295
T=246: 1 seg w=162
-> T_floor=240  T_shoulder=252  band=11  =>  T_op = 246
-> 6/6 homed, datum scatter sd 2.43 usteps = 0.0046 deg
```

Independent empirical optimum was 246–248, and adversarial runs at both gave
16/16 from starts spread around the whole circle. The rule lands on the right
answer without being told the answer.

## Cost

Phase 0 ~2 s + Phase 2 ~8 s, on **cold homes only**. Warm homes reuse the
cached `T_op`/`W_cal` and are unchanged (4–6 s). Comparable to the old cold
calibration.

## Caveat this design does not solve

The band gate will correctly **refuse to home** on an enclosed module, where
the band is 2 counts. That is the right behaviour — better than homing
unreliably — but it means an enclosed unit does not work. Fixing that is
optical, not algorithmic: raise the reflector albedo until the flag's crossing
is ≤ 230 measured **in the dark**, which gives a ~23-count band and makes the
enclosure question irrelevant.
