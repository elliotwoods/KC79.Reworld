# Axis B "fails to calibrate" — module serial 226, bench, 2026-09-08

**Short version.** The sensor and the painted flag on axis B are fine: the flag was seen on every
homing attempt and on every one of 39 census laps. The module failed startup (126 s, both axes,
`Routines.init: Fail on calibrate`) for two unrelated reasons, neither of them optical:

1. **The flags on this module are about twice as wide as the firmware's hard-coded fleet band
   allows** — on *both* axes. At the seed threshold T=235 the precise passes measured W≈565 (A) and
   W≈630 (B) against `FASTHOME_W_MAX = 520`, a constant derived from one reference module
   (258–275). Every path — warm, seeded, cold — found the flag cleanly and was then refused by that
   one ceiling. Axis A failed exactly the same way; B is just the axis the log (and the LED
   pattern) leaves you looking at.
2. **Axis B loses steps at the routine's 24,000 µsteps/s seek/reposition speed.** Measured by
   census: B's flag came back 800–11,235 µsteps *late* per revolution at 24 k (one lap missed it
   entirely), while A held to ±200 µsteps on every lap, and B itself held to ±200 at 14 k and 8 k.
   This did not cause today's failure, but it is a real, axis-B-specific weakness that the same
   routine relies on for its final park-onto-datum move.

Both are addressed in `PortalFW/src/Modules/MotionControl.cpp` (uncommitted): `FASTHOME_W_MAX`
520 → 1000, and the 32:1 `seekSpeed`/`reposSpeed` 24,000 → 14,000. Flashed as
`Portal v2026-09-08_19.57 6515b1d+`; startup now completes in **64 s with both axes homed**, and
8/8 follow-up homes succeeded. **But B's datum repeats to only ±0.7° (A: ±0.01°)** because B still
loses steps in short 14 k moves when hot — that is a mechanical/torque problem on axis B, and it
is what actually needs fixing on this unit (§6, §7).

The bench's "no ST-Link" is a third, separate thing (§1).

---

## 1. Why the bench "failed to see the ST-Link"

Two ST-Link V2-1 probes are on the USB tree: `066AFF…4219` (the old Portal-ID-2 bench probe) and
`066CFF…4217` (this module; its VCOM is `/dev/cu.usbmodem5103`). `flash.rs::adopt_selector`
adopts a probe only when exactly one is attached, so the bench sat at
`detail: "multiple ST-Links found; choose the fixture probe"`, `probe_connected: false` — which the
page renders as no probe. Selecting the `066CFF…` row fixed it immediately: MCU connected, identity
read (provision serial 226, on-board calibrations A T235/W407, B T235/W469).

Two side findings from `ioreg`:

- `Google Chrome` and `ChatGPT` each hold an `AppleUSBHostDeviceUserClient` on **both** ST-Links,
  and the `066AFF` device has *no interfaces enumerated at all* (no ST-Link Debug interface, no
  VCP node) — the signature of a WebUSB page having opened it exclusively. It lists, but nothing
  could open it. Worth closing whatever tab that is.
- macOS `sharingd` is listening on `*:8770`, so the bench started on a fallback port
  (127.0.0.1:52363 this session). The app window follows it; curl to 8770 does not.

## 2. What the board itself said (before any change)

Message outbox after the failed startup (firmware `v2026-08-26_13.52 05a7bf5+`):

```
[MotionControl_A] fastHome begin (cold, T=0, W_cal=0)
[MotionControl_A] bg guard: background censored (1/3 measurable), T_cap=253
[MotionControl_A] acquired at T_cap=253 lead=756708
[MotionControl_A] flag: lead=756708 span@cap=1390 crossing=223@757403 ceiling=253 usable=30 (min 2)
[MotionControl_A] operating point: T_op=240 W_cal=707 (crossing=223)
[E MotionControl_A] fastHome result: status=fail class=feature-missing reason=operating point out of band after passes: T=240 W=715
[MotionControl_B] fastHome begin (warm, T=235, W_cal=469)
[MotionControl_B] acquired at T=235 lead=379586
[E MotionControl_B] fastHome result: status=fail class=feature-missing reason=operating point out of band after passes: T=235 W=629
[MotionControl_B] fastHome begin (default, T=235, W_cal=266)
[E MotionControl_B] seed width gate: w=627 outside band [120..520]
[E MotionControl_B] fastHome result: status=fail class=feature-too-wide reason=seed feature width implausible
[MotionControl_B] fastHome begin (cold, T=0, W_cal=0)
[MotionControl_B] bg guard: background censored (1/3 measurable), T_cap=253
[MotionControl_B] flag: lead=381282 span@cap=1422 crossing=227@381756 ceiling=253 usable=26 (min 2)
[MotionControl_B] operating point: T_op=241 W_cal=855 (crossing=227)
[E MotionControl_B] fastHome result: status=fail class=feature-missing reason=operating point out of band after passes: T=241 W=857
[E Routines.calibrate] Fail
[Routines.init] Duration: 126s
```

Read it as: the flag is acquired every time, its crossing (223/227) is a normal painted-flag
brightness (the reference measured ~230), two precise passes agree, backlash is sane (A: −12,
clamped) — and the only thing that ever says no is the `(T, W)` band check, because W is 629–857.

`:d` at the same moment: `MotionControl_B: ACTIVE, crossing=241` — B was parked on its flag and
the comparator was reading it.

## 3. Census: width vs threshold, this module vs the reference

`:n <T> 0 <axis>` — one full revolution per threshold, every comparator transition reported,
24,000 µsteps/s. Every lap that saw the flag saw **exactly one segment** (no dither splits).

| T | 200 | 210 | 220 | 226 | 230 | 235 | 240 | 245 | 250 | 253 | 255 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| **A here** | – | – | 53 | 331 | 362 | **565** | 742 | 918 | 1266 | 1360 | 1933 |
| A reference | – | 48 | 53 | 185* | 185 | 275 | 357 | 432 | 973 | – | 1896 |
| **B here** | 210 | 192 | 347 | 583 | 574 | **650** | (lap missed, slip) | 2998† | 1589† | 1918† | 1709† |
| B reference | – | 53 | 193 | 53* | 260 | 258 | 300 | 330 | 866 | – | 1671 |

\* reference at 225. † B's readings above 235 are inflated by step loss during the lap (§4).
Repeat laps on B at T=222: 352, 448, 466; at T=226: 467, 430, 447 (the first 226 lap read 583).

The two modules have the same flag *brightness* (crossing 223–230) and the same shape of curve; this
one is simply ~2× wider at every threshold on both axes — a longer painted patch, not a fainter one.
The band `[T 226..246] × [W 120..520]` has no point in it for this module: at the lowest allowed T
the width is already 583 on B. Note also that the record on the board from the last time it homed
successfully was A W407 / B W469 — the flags have grown 30–40 % since then (see §7).

## 4. Census: axis B loses steps at 24,000 µsteps/s

Because each census lap starts where the previous one ended, the flag's leading edge should advance
by exactly one revolution (189,704 µsteps) between laps. Deviation from that is motion the counter
recorded but the ring did not make.

```
axis  T   speed   lead      delta-from-prev-lead   slip
B    210  24000     379288     189846 (1 rev)      +142
B    220  24000     568993     189705 (1 rev)        +1
B    226  24000     766237     197244 (1 rev)     +7540
B    230  24000     960300     194063 (1 rev)     +4359
B    235  24000    1152687     192387 (1 rev)     +2683
B    240  24000   (no flag in lap — slip had carried it 487 µsteps past the lap end)
B    245  24000    1349493     196806 (1 rev)     +7102
B    250  24000    1540641     191148 (1 rev)     +1444
B    253  24000    1730016     189375 (1 rev)      -329
B    255  24000    1927678     197662 (1 rev)     +7958
B    222  24000    2119031     191353 (1 rev)     +1649
B    222  24000    2314342     195311 (1 rev)     +5607
B    222  24000    2506497     192155 (1 rev)     +2451
B    226  24000    2697015     190518 (1 rev)      +814
B    226  24000    2890704     193689 (1 rev)     +3985
B    226  24000    3091643     200939 (1 rev)    +11235
B    226  24000    3282916     191273 (1 rev)     +1569
B    226  24000    3483603     200687 (1 rev)    +10983
B    226  14000    3679138     195535 (1 rev)     +5831   <- tail of the previous 24k lap
B    226  14000    3868645     189507 (1 rev)      -197
B    226  14000    4058458     189813 (1 rev)      +109
B    226   8000    4248069     189611 (1 rev)       -93
B    226   8000    4437982     189913 (1 rev)      +209

A    226  24000     755975     189883 (1 rev)      +179
A    230  24000     945709     189734 (1 rev)       +30
A    235  24000    1135373     189664 (1 rev)       -40
A    240  24000    1325041     189668 (1 rev)       -36
A    245  24000    1514804     189763 (1 rev)       +59
A    250  24000    1704305     189501 (1 rev)      -203
A    253  24000    1893994     189689 (1 rev)       -15
A    255  24000    2083701     189707 (1 rev)        +3
A    226  14000    2273904     190203 (1 rev)      +499
A    226  14000    2463649     189745 (1 rev)       +41
```

B at 24 k: 14 of 16 laps slipped, 0.4–6 % of a revolution each, worse as the motor warmed. B at
14 k and 8 k: within ±200 on every lap. A at 24 k: within ±200 on every lap. Same driver settings,
same current (250 mA), same threshold — this is axis B's torque margin at speed, not software.
The startup cycle check (which runs at 14,080) passed B at −298 µsteps/rev, consistent with this.

Why it matters even though today's failure was the width band: `fastHomeRoutine` used 24,000 for
the seek *and* for every reposition, including the final park onto the datum. A slip there leaves
the axis short of home with the counter saying it arrived — a silent datum error for the whole
session. (The 16:1 bench notes recorded exactly this as a ~780 µstep park error.)

## 5. Changes made (`PortalFW/src/Modules/MotionControl.cpp`, uncommitted)

```
-	#define FASTHOME_W_MAX             520
+	#define FASTHOME_W_MAX             1000
-		{ 32, 24000, 100000, 24000, 2000, 32, 5000, 2000, 4200, 3000, 189704 };
+		{ 32, 14000, 100000, 14000, 2000, 32, 5000, 2000, 4200, 3000, 189704 };
```

- **W_MAX 1000.** The smear this ceiling exists to reject begins at T≥250 (866–973 on the
  reference, 1266+ here) and is already excluded by `FASTHOME_T_OP_MAX = 246`; within T≤246 the
  widest honest reading on any module is 918 (A at 245). 1000 keeps a real ceiling and admits a
  generously painted flag. The same predicate guards caching, persisting and restoring, so all
  four agree.
- **Seek/repos 14,000.** `MOTION_DEFAULT_SPEED`, the speed every module runs at all day and the
  speed the cycle check already trusts. Cost: a warm home ~3 s longer, a cold one ~6 s. Side
  benefit visible immediately: the seek's latch error against the 2 k creep (`seekErr`) went from
  hundreds–thousands of µsteps to −4 (A) and +48/+52 (B).

Both comments in the source carry the measurements above. Built with
`node tools/build-firmware.mjs --env application_bank_optical` (100,732 bytes, 7,812 free),
flashed application-only from the bench (`flash passed: verified firmware a403ed24c1cb; identity
preserved serial 226; settings unchanged at 250 mA`), bootloader left untouched.

## 6. After the change

Boot log, first startup on the new firmware:

```
Optical Calibration: A=flash T235/W407; B=flash T235/W469
[Routines.cycleCheck] A: pass | B: pass          (B: 189406 usteps, -298; A: 189669, -35)
[MotionControl_A] fastHome begin (warm, T=235, W_cal=407)
  pass 0: lead=355603 trail=356134 w=531 mid=355868 seekErr=-4
  pass 1: lead=355603 trail=356134 w=531 mid=355868
  backlash: trail=356134 reenter=355594 -> 540
fastHome OK: datum=355868 w=531 backlash=540 T=235 (warm, 8s)
[MotionControl_B] fastHome begin (warm, T=235, W_cal=469)
  pass 0: lead=242999 trail=243758 w=759 mid=243378 seekErr=148
[E MotionControl_B] width gate: w=759 outside [304..633]      <- stale W469 record, correctly rejected
[MotionControl_B] fastHome begin (default, T=235, W_cal=266)
  pass 0: lead=433010 trail=433789 w=779 mid=433399 seekErr=48
  pass 1: lead=433006 trail=433789 w=783 mid=433397 seekErr=52
  backlash: trail=433789 reenter=433117 -> 672
fastHome OK: datum=433398 w=781 backlash=672 T=235 (default, 22s)
[PersistentStorage] optical settings committed: gen=7 mask=3 source=flash-b
[Routines.init] Duration: 64s
```

Repeatability campaign (`:h 1 5` on B, `:h 0 3` on A, warm, back to back):

| axis | run | pass 0 (lead / trail / w / mid) | pass 1 (lead / trail / w / mid) | datum | err vs 1 rev | backlash |
|---|---|---|---|---|---|---|
| B | 1 | 189231 / 189966 / 735 / 189598 | 189231 / 189970 / 739 / 189600 | 189599 | −105 | 688 |
| B | 2 | 189068 / 189755 / 687 / 189411 | 188984 / 189687 / 703 / 189335 | 189373 | **−331** | 504 |
| B | 3 | 189776 / 190479 / 703 / 190127 | 189776 / 190483 / 707 / 190129 | 190128 | **+424** | 620 |
| B | 4 | 189074 / 189805 / 731 / 189439 | 189074 / 189805 / 731 / 189439 | 189439 | −265 | 440 |
| B | 5 | 189518 / 190209 / 691 / 189863 | 189382 / 190089 / 707 / 189735 | 189799 | +95 | 728 |
| A | 1 | 189422 / 189977 / 555 / 189699 | 189422 / 189977 / 555 / 189699 | 189699 | −5 | 508 |
| A | 2 | 189366 / 190025 / 659 / 189695 | 189370 / 190033 / 663 / 189701 | 189698 | −6 | 532 |
| A | 3 | 189388 / 190043 / 655 / 189715 | 189376 / 190055 / 679 / 189715 | 189715 | +11 | 556 |

Each run starts parked on the previous datum, so a perfect re-home reports `datum = 189,704`
(one revolution) and the column "err" is the run-to-run datum error. All eight homes succeeded, at
T=235, 22 s each (a full-lap seek at 14 k).

- **A: spread 17 µsteps, sd 8 — 0.03°.** The class the bench work reported for a healthy 32:1
  axis.
- **B: spread 755 µsteps, sd 273 — 1.4° peak to peak.** Forty times worse, and it is not the
  sensor: in runs 2 and 5 the *leading edge itself* moved 84 and 136 µsteps between the two
  passes of one run (`lead` 189068 → 188984; 189518 → 189382), i.e. the ring moved against the
  step counter during the ~2.7 k µstep reposition between passes — at 14 k, hot. Where B did not
  slip between passes (runs 1, 3, 4) its passes agree to 4 µsteps, exactly like A.

So after the change B **calibrates every time**, but its datum is only good to roughly ±0.7° until
the mechanical cause of the step loss is found. A is production-grade.

Static view afterwards (`:t` → 235; `:d`): both axes parked on their flags, `A: ACTIVE,
crossing=220`, `B: ACTIVE, crossing=183`. Note that B number: the cold-run probe measured B's
brightest crossing at 227 an hour earlier, and its start-of-session on-flag reading was 241. B's
flag is reading ~45 counts brighter at the end of the session than at the start, while A moved
223 → 220. That is the same drift the width shows (469 → 650 → 781), and it is on the axis whose
motor spent the session stalling — most likely thermal, but it is a second reason to re-measure B
cold.

## 7. What is still worth your attention

1. **B's flag width is drifting upward during the session**: on-board record 469 → 627–650 at the
   start of the session → 759–783 forty minutes later, while A moved 407 → 565 → 531. That is the
   documented thermo-optical "breathing", but B's amount is large, and it was measured after B's
   motor had been driven hard (slipping = stalling = heat). Worth a cold re-home tomorrow to see
   where B settles; if it keeps climbing towards 1000 the new ceiling will bite again, and that
   would be a sensor/geometry question for this unit.
2. **B's torque margin.** Nothing in software makes a motor slip at 24 k while its twin does not.
   Same current, same microstepping, same driver settings. Look at B's gear mesh / prism friction /
   motor, and compare the two by hand under load. The firmware no longer depends on 24 k, but a
   motor that stalls at 1.7× cruise is closer to its limit than the design assumed.
3. **Why are these flags 2× wider than the reference module's?** Almost certainly the painted
   patch is longer. If that is the intended production paint, the reference census in
   `MotionControl.cpp` (and `FASTHOME_W_DEFAULT = 266`) describes the *old* paint and the fleet
   constants should be re-derived from a couple more modules; if it is over-paint, the band would
   have caught it, which is arguably what it was for. Either way the 1000 ceiling is safe: the
   smear is excluded by the threshold clamp, and the two-pass repeatability and backlash gates
   still stand.
4. `routineMoveTo` prints `\r<position>\t` to USART1 on every 1 ms loop pass — ~90 kbit/s of a
   115 kbit/s link during any routine. It is pre-existing and did not cause anything here, but it
   makes the debug log hard to read (every census/edge line arrives glued to a run of positions)
   and it is the first thing to remove if a routine ever looks timing-sensitive.
5. The census remains the trustworthy instrument, but its per-lap position drift is the only way
   step loss shows up anywhere — nothing in the firmware measures slip during homing. A cheap
   guard: after the final park, creep back onto the leading edge and compare with `lead`.

## Reproducing the measurements

- VCOM held open by a tiny termios daemon (`vcomd.py`, session scratchpad) so nothing toggles the
  line between commands; `:n T 0 axis` per lap; `:h axis N` for campaigns; `:d` for the static
  view. All raw output is in the session's `vcom.log`.
- Bench driven over CDP against the visible app (probe row click, "Keep existing bootloader",
  flash button twice), as in the `bench-drive-live-via-cdp` note.
