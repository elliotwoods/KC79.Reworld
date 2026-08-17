# Silver-painted home feature vs unpainted — side A, 32:1 (2026-08-17)

Paint is an **optional** manufacturing step. The unpainted ring already homes
3,593/3,593 (`SESSION.md`), so the question is narrow: does painting the home
feature silver measurably improve anything worth paying for?

**Answer: it buys ~5× detection margin, and nothing measurable in home
repeatability.**

## 1. Backward compatibility

At the stored unpainted threshold **T=247**, the painted feature still homes
(`H,1,...,"ok"`) and gives one clean census segment. The flag is **1078 µsteps
wide vs 163 unpainted** at the same threshold — a 6.6× jump, the first sign the
albedo moved a long way.

## 2. Optical profile (full 0–255 sweep)

Crossing duty is inverse to reflectance. Instruments: static level ladder
(park, step T, read live comparator level) and census width-vs-T. The `K` grid
scan is *not* used — it sweeps the RC-filtered DAC and reads 10–20 counts high.

| | unpainted | **painted** |
|---|---|---|
| flag crossing at segment centre | ~240–245 | **230** |
| lowest T with detection (`T_floor`) | 240–242 | **~200** (nothing at 190, 94 µsteps at 200) |
| `T_shoulder` (segment splits) | 252 | **253** |
| **detectability band** | **9–11 counts** | **~52 counts** |
| background | censored (>255) | **censored (>255) — unchanged** |

Width vs threshold (census, one lap each):

| T | 200 | 208 | 214 | 218 | 222 | 226 | 238 | 250 | 251 | 252 | 253 | 254 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| segs | 1 | 1 | 1 | 1 | 1 | 1 | 1 | 1 | 1 | 1 | **2** | **7** |
| width | 94 | 203 | 225 | 286 | 316 | 352 | 789 | 1410 | 1522 | 1795 | 59+1847 | splits |

Two things worth noting:

- **No specular pathology.** Silver is specular and I specifically looked for a
  glint spike or a double-peaked profile at the patch edges. There is none — one
  clean segment from T=200 to T=252, splitting only at the shoulder (253+),
  which is the background arriving and is the same behaviour as unpainted.
- **A low-sensitivity zone at T≈208–226**, where width grows only ~8 µsteps per
  count, versus ~55/count above 226. That is the patch core with steep flanks;
  above 226 the soft surround starts contributing.

## 3. Threshold selection — measured, not assumed

Rather than assume band-centre or steepest-flank, fixed-start repeatability
(`repeat_home.py`, constant gear engagement so backlash is held out) was
measured at nine thresholds spanning the band:

| T | 205 | 214 | 218 | 222 | 224 | 226 | 228 | 230 | 234 | 240 | 250 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| datum sd (µsteps) | 2.35 | 2.60 | 0.45 | 4.69 | 0.58 | **0.29** | 2.84 | 2.23 | 4.34 | 4.81 | 4.72 |
| width | 165 | 236 | 279 | 308 | 335 | 355 | 427 | 488 | 778 | 876 | 1456 |
| width sd | 4.6 | 5.5 | 1.4 | 7.0 | 0.3 | 0.5 | 5.7 | 1.0 | 8.4 | 10.9 | 0.5 |

T=226 looked like a spectacular sweet spot — sd 0.29 µsteps (0.0006°), ~10×
better than unpainted. **It did not reproduce.** Re-run at n=25:

```
T=226 n=12:  datum sd 0.29   width 355 sd 0.5
T=226 n=25:  datum sd 4.16   width 388 sd 7.1
```

So the apparent structure across the table is **sampling noise** — an sd from
n=12 carries roughly ±20% of itself, and the run-to-run spread here is far
larger than that, because the flag width also drifts (355 → 388, +9%, over
~20 min of thermo-optical drift). Pooled across all thresholds and runs, painted
fixed-start datum sd is **~2.8 µsteps**; unpainted was **~2.65**. Statistically
indistinguishable.

**Chosen operating point: T = 226** — band centre (~26 counts of margin each
side) and at the top of the low-sensitivity width zone. It is not chosen for
superior repeatability, because no threshold has any.

## 4. Results at T=226

- **20/20** adversarial homes from starts around the whole circle, alternating
  approach; width 361, sd 13.4.
- **4/4** census laps, exactly one segment, repeatable position (~0.23° spread,
  which is census-latch coarseness, not homing error).
- **~190 homes this session across all thresholds, zero failures.**

## 5. Comparison

| metric | unpainted | painted | verdict |
|---|---|---|---|
| detectability band | 9–11 counts | **~52 counts** | **paint wins, ~5×** |
| flag width at T_op | 163 | 355–411 | paint wins (comfortable margin over gates) |
| fixed-start datum sd | 2.65 µsteps ≈ 0.005° | 2.8 µsteps ≈ 0.005° | **no difference** |
| specular artefacts | n/a | none found | no penalty |
| reliability | 3,593/3,593 (18 h) | 190/190 (1 session) | not comparable — no overnight |
| background | censored | censored | unchanged; `T = bg − k` still impossible |

## 6. Recommendation

**Do not paint for the open / ambient-lit configuration.** The unpainted ring
already homes 3,593/3,593 with a band that held 9–11 counts across a full 18 h
dark/light cycle and a `T_shoulder` that never moved. Paint adds a
manufacturing step and buys margin that configuration does not need. It does
*not* buy a better home position — that was the hoped-for advantage and it
is not there.

**Paint is the right answer if modules are enclosed.** That is the one case
where unpainted is genuinely marginal: with a physical cover the unpainted band
collapses to **2 counts** (T=252–253) and T=253 goes bimodal. A 5× larger band
is exactly the remedy for that, and it would very likely keep an enclosed sensor
comfortably above the 6-count gate.

**The decisive experiment has not been run**: painted band *with the cover
fitted*. It needs the operator to refit the cover and takes ~5 minutes. Until
then the case for paint rests on an inference, not a measurement.

## 7. Caveats

- Repeatability here is fixed-start (constant gear engagement). Datum accuracy
  under varied approach remains backlash-dominated and unresolved for **both**
  variants until the band-centred calibration is ported into `O` with its
  backlash compensation.
- 190 homes in one session cannot be compared with a 3,593-home overnight. If
  paint is adopted, bake it overnight before committing.
- One ring, one rig, one thermal state.
