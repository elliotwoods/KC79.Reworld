# Optical threshold strategy — the defence layer (2026-08-25)

Continuation of [`HOME_ROUTINE_DESIGN.md`](HOME_ROUTINE_DESIGN.md). That document designed how
the operating comparator threshold is *chosen*; this one is about how a chosen `(threshold,
width)` pair is *defended* — screened wherever it can be adopted, persisted, or restored — and
records the firmware as it actually stands after this session, including one design item that was
deliberately **not** shipped and why.

All symbols below are in `PortalFW/src/Modules/` unless noted. This is production firmware, not
the bench (`HomeSwitchTest/`).

## The flag-detection path (orientation)

Three routines detect the reflective home flag, in increasing precision. Each is documented in the
code; this is the map:

1. **`cycleCheck`** (`Routines::cycleCheck`, `MotionControl::cycleCheckBegin/Update/End`) — the
   startup fast check. **Both axes at once**, at the seed threshold (235), verifying the flag comes
   past **exactly once per revolution** — a second sighting arriving early is itself a failure.
   Coarse: it answers "does this prism turn and show one clean flag", not "what is the exact
   operating point". Runs *before* calibration, so a jammed/optically-dead axis is named in seconds;
   a failure aborts startup so the idle LED fault pattern shows which axis is bad.
2. **`fastHomeRoutine`** — per-axis, serial (the DAC is shared). Selects the operating threshold
   ([`HOME_ROUTINE_DESIGN.md`](HOME_ROUTINE_DESIGN.md)), measures the flag width and backlash, sets
   the datum. Warm/seeded is the fast common case (~8 s); the cold measure-everything path is the
   rare fallback.
3. **The defence band** (the rest of this document) — screens every `(T, W)` pair the above produce,
   cache, persist or restore, so a smeared or marginal point can never be adopted or trusted.

## Why this layer exists — the incident

A module (provision serial 9) carried a **persisted calibration of T=247 / W=881** on axis B. That
is a *smear*: the threshold sat in the 250-adjacent region where the flag grows into the surround
(the census table in `MotionControl.cpp` shows W jumping past 800 at T≥250 against ~260 at 235).
Nothing screened it. On boot the firmware trusted it as "warm", the first precise pass measured a
sane W=363, the warm width gate rejected 363 for disagreeing with the stored 881 (`±35 %` of the
wrong number), and the axis fell through warm → seed → cold over **~150 s** before recovering a
good pair. The failure was not bad luck; it was an unguarded path — **no gate anywhere related the
width to the threshold.**

## The model — one clean band, one predicate

The width-vs-threshold curve (census table, `MotionControl.cpp`, the `FASTHOME_T_DEFAULT` comment)
has three regimes on the painted production ring:

```
T     210 215 220 225 230 235 240 245 250 255
A W    48  53  53 185 185 275 357 432 973 1896
B W    53  49 193  53 260 258 300 330 866 1671
        └── too narrow ──┘ └─ clean ─┘ └ smear ┘
```

A `(T, W)` pair is trustworthy only in the clean plateau. Datum *quality*, not just detection,
collapses at the smear end (2× width, 2.5× backlash, 10× worse repeatability — see `PORTING.md`
item 7 and `SESSION.md`). So the band is encoded as fleet constants and one predicate:

```
FASTHOME_T_OP_MIN  226      // below: narrow sliver, marginal detection
FASTHOME_T_OP_MAX  246      // above: smear knee at 250 (W>800)
FASTHOME_W_MIN     120
FASTHOME_W_MAX     520      // 432 @ T=245 is top-clean; 866 @ 250 is smear

MotionControl::opticalPointPlausible(int T, Steps W)   // T in [MIN..MAX] && W in [MIN..MAX]
```

**Fleet constants, not per-module.** They assume the painted production optics; a materially
different ring needs them re-derived from a fresh census (`:n`). The per-axis warm cache still
tightens the width gate to the axis's *own* measured width after its first success, so day/ambient
drift is absorbed without a provisioning step. This was a deliberate choice over storing a
per-module band.

## Where the predicate is enforced

| Gate | Where | Effect |
|---|---|---|
| **Persist** | `App::persistOpticalCalibration` | a pair outside the band is refused before it reaches flash — the incident fix |
| **Restore** | `MotionControl::restoreOpticalCalibration` | an implausible flash record is ignored at boot; the axis comes up uncalibrated (seeded path) instead of trusted-warm against a bad width |
| **Success cache** | `fastHomeRoutine`, the `opticalThresholdCached = T` block | a run that somehow ended out of band does not poison the cache/persist |
| **Seed width gate** | `fastHomeRoutine`, seeded first pass | was dead code (`FASTHOME_SEED_WIDTH_LO/HI` computed and never read → effective `[8..4200]`); now gates against the band |
| **Cold `T_op`** | `fastHomeRoutine`, after `T_op = C_flag + round(0.55·usable)` | `T_op` is clamped into `[max(C_flag+1, T_OP_MIN) .. min(T_OP_MAX, T_cap)]`; if the flag only resolves where those windows don't overlap it **fails honestly** (`"no clean operating point below the smear"`) rather than adopting a smear. This band, not `FASTHOME_MARGIN_MIN=2`, is the real guard on the operating point now. |

## Self-heal, and the version bump

A failing warm-from-flash attempt clears the RAM cache but **never used to touch the flash record**,
so a bad record reloaded and re-failed every boot. Now the restore gate rejects it at boot (→
seeded path), and the next successful home overwrites it via `persistOpticalCalibration`.
`opticalCalibrationVersion` is bumped **1 → 2** so a pre-gate record does not dedup-match and is
re-earned once under the new firmware.

## Shared DAC park (implements `PORTING.md` item 8)

One RC-filtered PWM on PC15 feeds both comparators. The `fastHomeRoutine` success path left the DAC
at whichever axis homed *last* — so axis A's comparator was read at axis B's threshold (247 in the
incident). `Routines::calibrate` now parks it at `min(T_A, T_B)` after both axes (default 235 if
either is uncalibrated). Verified: `:t` reads 235 after a two-axis calibrate.

## What was NOT shipped — the one-segment verification lap, and why

`HOME_ROUTINE_DESIGN.md` Phase 1 specifies *"require exactly one span per revolution; more than one
→ fail"*. It has never been implemented in firmware, and a lap to verify it at `T_op` was written
this session and then **removed**. Reason, found on hardware:

> **`homeSwitchCensusRoutine` (`:n`) is position-fragile.** From a clean position ~19k µsteps before
> the flag it reports correctly (`segs=1, widest=299` at T=235). From the home datum it reported
> **`segs=0` for a flag that is demonstrably there** (both axes home fine at 235). Home parks *off*
> the flag — `:d` shows both axes `inactive, crossing=censored` at the datum — so the census
> starting near the datum/flag boundary misses it.

A one-segment gate built on that primitive could **false-fail a good cold calibration** and throw
the axis into the very ~150 s cascade this work exists to prevent. The trade is backwards, so it
was pulled. The "one flag per revolution" guarantee is still provided for the production fleet by:

- the startup **`cycleCheck`** (`Routines::cycleCheck` / `MotionControl::cycleCheck*`), which enforces
  exactly one flag per revolution at the seed threshold (235 ≈ the operating threshold for the whole
  painted fleet) *before* calibration begins; and
- the cold `T_op` band clamp keeping the operating point ≤ 246, structurally clear of the 253–255
  dither zone where segment splits were ever observed (the census recorded "exactly ONE segment
  everywhere, including at 255" on painted rings).

**Re-enabling a per-`T_op` one-segment check requires fixing the census first** — see open items.

## Verification (this session)

- **Incident regression, end to end.** The original smear (T=247/W=881, as an `optCalVer=1` record)
  was written back into flash over SWD and the module booted. Result: `ignoring implausible
  persisted calibration T=247 W=881` → axis B seeds → homes in 7 s → **45 s total** (vs the ~150 s
  cascade the identical state caused before) → self-healed to a gen-11 record `B: T235/W237,
  optCalVer=2`, both axes in band.
- **Normal warm startups**: 40–45 s, both axes T=235, no false-trips from any new gate.
- **DAC park**: `:t` = 235 = `min(T_A, T_B)` after calibrate.
- **Good operating points, current painted fleet**: A T235 / W≈300–345, B T235 / W≈207–245.

## Open items for a future session

1. **The census position-fragility is a real, pre-existing bug** — it also affects the operator `:n`
   diagnostic. Start at the first-move arming in `homeSwitchCensusRoutine`
   (`updateStepsAndSwitches` / `inInterrupt.invertSwitches` timing at lap start), and at the fact
   that the lap can begin on or just past the flag. Fixing it unblocks a proper per-`T_op`
   one-segment verification.
2. **The cold path was not hardware-exercised.** Cold only runs on an axis where the fleet seed
   (235) fails outright, which a good module never does. The changes are simple arithmetic against
   the in-source census table; a genuinely marginal/optically-dead module (e.g. the serial-8 unit
   seen this session) would exercise the `"no clean operating point below the smear"` fast-fail
   directly.
3. **Band bounds are fleet constants.** If the ring albedo/paint changes, re-derive
   `FASTHOME_T_OP_*` / `FASTHOME_W_*` from a fresh census across T=175..255 on both axes.
