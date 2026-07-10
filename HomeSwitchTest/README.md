# HomeSwitchTest

Standalone bench tooling for the optical home switch
(`Modules::HomeSwitchOptical`). Lives **outside** PortalFW so the production
firmware stays pristine; it reuses the real switch class (and its TIM6
software-PWM threshold "DAC") rather than copying it.

Two firmwares, selected by PlatformIO env:

| env | entry point | purpose |
|-----|-------------|---------|
| `home_switch_test`  | `src/main.cpp`         | **Sweep / calibration** — finds each axis's threshold crossing. Uses the OLED. |
| `home_switch_track` | `src/tracker_main.cpp` | **Fixed-threshold tracker** — reads A/B live, streams a 1-byte frame at 60 Hz. |
| `home_switch_bench` | `src/bench_main.cpp`   | **Unified Side-A rig** — live sensor + settable threshold + motor jog/goto + homing, driven by the `home_switch_gui.py` GUI. Adds motor control. |

Shared source, no duplication: `src/Modules` is a symlink to
`../../PortalFW/src/Modules`; each env's `build_src_filter` compiles only its own
entry point plus `Modules/HomeSwitchOptical.cpp` + `Modules/Base.cpp`. The local
libs (`msgpack-arduino`, and `u8g2stm32`/`U8g2` for the sweep tool only) come from
`PortalFW/lib` via `lib_extra_dirs`.

The Python monitors share one venv:

```sh
python3 -m venv monitor/.venv
monitor/.venv/bin/pip install -r monitor/requirements.txt
```

---

## Sweep tool (`home_switch_test`)

Continuously sweeps the shared comparator threshold (PC15) **ascending only**
(hysteresis ignored) and records, per axis (A=PC13, B=PC14), the duty at which the
comparator output first flips — a proxy for the sensor's analog level.

- **OLED**: live per-axis crossing duty + threshold voltage + rolling σ + sparkline.
- **Serial** (USART1, PB6/PB7 @ 115200), one CSV line per sweep:
  `D,<millis>,<A_duty>,<B_duty>,<A_sigma_x10>,<B_sigma_x10>,<A_lo>,<A_hi>,<B_lo>,<B_hi>`
  (`duty=-1` = no crossing; `*_lo`/`*_hi` = comparator state at range ends, 1=HIGH).
- **`monitor/home_switch_monitor.py`**: matplotlib live graph of A/B over time.

```sh
~/.platformio/penv/bin/pio run -e home_switch_test -t upload
monitor/.venv/bin/python monitor/home_switch_monitor.py   # --port … --csv … --window …
```

> Accuracy: the threshold DAC is RC-filtered (τ≈100 ms), so a fast sweep reads a
> duty offset *high* from the true crossing. The offset is systematic — calibrate by
> taking the **midpoint** of the crossings in the "home" and "not-home" target
> positions (which must be ≫ σ apart), and write it into
> `HOMESWITCHOPTICAL_DEFAULT_THRESHOLD` (`PortalFW/.../HomeSwitchOptical.h`).

---

## Tracker (`home_switch_track`)

Holds the threshold **fixed** at `HOMESWITCHOPTICAL_DEFAULT_THRESHOLD` (currently
220) and reads both comparator outputs as fast as possible — no sweeping, so it
tracks the live home-switch state in realtime.

- **LEDs** track the live state every inner-loop pass (effectively instant):
  PB3 → axis A, PB4 → axis B.
- **Serial** streams **one byte per frame at 60 Hz** (no OLED, to keep the cadence):

  | bit | 7 | 3 | 2 | 1 | 0 |
  |-----|---|---|---|---|---|
  | meaning | sync (always 1) | B edge | A edge | B level | A level |

  *edge* = that axis changed since the previous frame, so a brief pulse between two
  60 Hz frames is still reported. The PC syncs on bit7; the ASCII boot banner
  (bytes < 0x80) is ignored by the reader.
- **`monitor/home_switch_tracker.py`**: matplotlib live logic-analyzer view — two
  scrolling A/B traces with edge tick-marks and measured frame rate.

```sh
~/.platformio/penv/bin/pio run -e home_switch_track -t upload
monitor/.venv/bin/python monitor/home_switch_tracker.py   # --port … --window …
```

---

## Bench tool + GUI (`home_switch_bench`)

The unified **Side-A** rig: one firmware + one cross-platform (Windows + macOS)
GUI that does live sensor reading, threshold calibration, motor jog/go-to, **and
the homing sequence** — the motor turns the ring gear via the Axis-A stepper so
the extrusion sweeps the sensor. Only Side A is touched; Side B is never driven.

It reuses the real driver classes (`MotorDriver`, `MotorDriverSettings`,
`HomeSwitchOptical`) plus a lightweight, self-contained motion controller
(`src/BenchMotion.{h,cpp}`) that does the same job as `MotionControl::homeRoutine`
without pulling in the App/Logger/keyframe stack. Homing takes **one slow forward
sweep** across the flag, latching the leading edge (sensor rising) and the
trailing edge (sensor falling) in that single pass, then sets home to the
midpoint and zeros there. Both edges are measured in the same forward-engaged
frame, so the centre is free of gear backlash. (`src/bench_log_stub.cpp` supplies
the one `log()` symbol `MotorDriver.cpp` needs so it links without the App-coupled
real `Logger.cpp`.)

- **GUI:** `monitor/home_switch_gui.py` — Tkinter + matplotlib. Needs `pyserial`
  and `matplotlib` (already in `requirements.txt`) plus **Tkinter**, which ships
  with the standard python.org CPython on Windows and macOS.

```sh
~/.platformio/penv/bin/pio run -e home_switch_bench -t upload
monitor/.venv/bin/python monitor/home_switch_gui.py   # --port … --baud …
```

### Line protocol (USART1, PB6/PB7 @ 115200)

Bidirectional; one `\n`-terminated ASCII line per message (also fine to type by
hand in a serial terminal).

**Host → firmware**

| cmd | meaning |
|-----|---------|
| `T <duty>` | set comparator threshold (0-255) |
| `M <0\|1\|2>` | mode: idle / track / sweep |
| `E <0\|1>` | motor coils off / on (holding torque) |
| `J <dµsteps> [speed]` | jog relative (signed), optional speed (µsteps/s) |
| `G <µsteps> [speed]` | go to absolute position |
| `V <speed>` | set default speed |
| `H` | run homing sequence (Side A) |
| `Z` | zero current position |
| `X` | abort current motion / routine |
| `P` | emit one status line now |
| `R <microcode> <mA>` | set microstep resolution code + coil current |
| `C <signedSpeed>` | continuous jog (non-blocking); 0 stops |
| `Q` | one threshold sweep at the current position |
| `F <N>` | coarse-to-fine peak search over ±N µsteps (keeps only the peak) |
| `K <N> [step] [dutyMin] [dutyMax]` | home-shape grid scan (dutyMin default 200, must be < the lowest real crossing) |
| `A <N> [step]` | self-calibrating two-edge dip home (finds its own threshold) |
| `O [vEdge] [M] [vSeek] [accel] [forceCal]` | **fast home + backlash** — the production-candidate routine (see below) |
| `N [T] [vmax] [accel] [M]` | full-rev sensor **census** at fixed threshold T: one ramped lap, dumps every debounced transition |
| `Y <dµsteps> [vmax] [accel]` | ramped (trapezoid) relative move — speed/accel probing |
| `W` | debug: 100-sample burst comparing digitalRead vs the ISR's direct register read |

**Firmware → host**

- `S,ms,level,thr,pos,deg_x10,running,enabled,fault,homed` — status (~60 Hz)
- `D,ms,A_duty,B_duty,A_sig_x10,B_sig_x10,A_lo,A_hi,B_lo,B_hi` — sweep (B unused)
- `H,ok,home,switchSize,leadingEdge,trailingEdge,"message"` — homing result
- `Q,pos,cross[,lo,hi]` — one crossing sweep (`Q`) / peak-search sample
- `B,bestPos,bestCross,threshold` — peak-search (`F`) result
- `K,begin,center,n,step,count,dutyMin,dutyMax` · `K,index,pos,cross,lo,hi` · `K,end,center,samples,aborted`
  — home-shape scan (`K`): one line per position, `cross`=−1 means no crossing,
  `lo`/`hi` = comparator rail state at dutyMin / dutyMax. `K,end` is sent even on abort.
- `A,begin,center,n,step,dutyMin,dutyMax` · `A,pt,index,pos,cross,lo,hi` (coarse point)
  · `A,thr,T,cmin,pmin,dipDepth` · `A,edge,0|1,pos` (leading/trailing)
  · `A,done,ok,home,lead,trail,switch,T,dipDepth,"message"` — auto-home (`A`).
- `O,begin,pos,T,vEdge,M,vSeek` (T=0 → will calibrate) · `O,cal,c1,c2,bg,T` ·
  `O,thr,T,up|down` (adaptive bump) · `O,seek,coarseLead` · `O,gate,name,pass,info` ·
  `O,edge,0|1,pos` · `O,backlash,µsteps,reenterPos` ·
  `O,done,ok,home,lead,trail,switch,backlash,T,ms,"message"` — fast home (`O`).
- `N,begin,pos,T,vmax,lap` · `N,edge,i,pos,state` · `N,end,count,aborted` — census (`N`).
- `L,level,message` — progress/log · `#…` — banner (announces `usteps_per_rev`)

### Procedure

1. Flash `home_switch_bench`, launch the GUI, click **Connect**.
2. Move the ring gear (by hand or **Jog**) so the extrusion crosses the sensor;
   the **FLAG PRESENT / absent** box and board LED PB3 should toggle.
3. Pick a threshold: drag the slider (applied live on release) until PRESENT vs
   absent separate cleanly, or use **Start sweep → Capture HOME / Capture
   NOT-home → Apply** to take the midpoint (margin should be ≫ sigma).
4. **Enable** the motor and **Jog ±** to confirm the gear turns and the position
   tracks; check the **FAULT** lamp stays grey.
5. Click **HOME**: it finds both flag edges and zeros at the centre. **Run
   repeatability** homes N× and reports the spread (want a small fraction of a
   degree). **STOP** aborts at any time.

> The homing **midpoint** is backlash-free (both edges are latched in one forward
> pass), but the bench controller has no backlash model, so the final "park at
> centre" move — and any later reversal — can sit ~½·backlash off. That's fine for
> validating the sensor and measuring repeatability; add compensation later if the
> spread demands it.

### Home-shape scan (`K`, wizard Step 2)

Before choosing a homing strategy, measure the actual shape of the home region.
Jog the flag over the sensor, then run a **grid scan**: the firmware sweeps the
motor across `±N` microsteps (default 500) in `step`-microstep increments (default
10, ≈101 positions). At each position it sweeps the comparator threshold **low→high
only** (one hysteresis branch) and records the **crossing duty** — the duty where
the sensor output flips, a proxy for optical reflection strength. Plotting crossing
vs. position reveals whether the home region is a **peak**, a **top-hat**, a
**top-hat with a dent**, etc., which dictates the homing strategy (edge-find vs.
precise centre-find).

- The whole grid is measured in one forward pass (approached from below to take up
  backlash), so the curve is on a single gear flank.
- The scan **does not** change the operating threshold (unlike `F`).
- A very strong peak can push the crossing past 255 → reported `cross=−1, hi=1`
  ("saturated"); the GUI marks those points red. Off-flag positions read
  `cross=−1, lo=hi=0` and show as grey ✕.
- Runtime ≈ 2–3 min for 101 positions; the `S` status stream and `X` abort stay
  live throughout. Tunables (`kScanDutyStep`, `kScanDwellMs`, `kScanSettleMs`) are
  in `src/bench_main.cpp`.

In the GUI (Step 2), set **range**/**step**/**duty min-max** (default 200–255 — no
real crossing occurs below ~200, so restricting the duty band cuts scan time ~4×),
click **Run scan** (watch the curve fill in live), **Stop** to abort, then **Save
scan…** to write a timestamped `.csv` (per-position table + metadata header), a
`.png` of the graph, and a `.json` sidecar.

### Auto-home (`A`, wizard Step 3)

On this hardware the home feature is a **dip** in crossing-duty (weak reflection at
the centre, ~236) between reflective shoulders (~255): at a fixed threshold `T` the
sensor reads ACTIVE where `crossing < T`, so the legacy operating threshold (220,
*below* the whole dip) can never see it. The `A` command **self-calibrates**: it
coarsely characterises the dip (restricted duty, ~20–30 s), picks a threshold into
the upper-middle of the dip (`T = cmin + 0.6·(255−cmin)`, ~247), then two-edge homes
— a threshold hit is an **edge**, not the centre, so it latches both the leading and
trailing edges in one forward pass and takes the midpoint (like the mechanical
switch). It adopts `T` as the live operating threshold and zeros at the centre. Uses
no values from the manual scan.

In the GUI (Step 3) click **Calibrate & home**: the coarse curve, the chosen
threshold (blue dashed), the two edges (red) and the home centre (green) draw live.
**Repeatability ×N** then re-homes with the fast `H` edge-home at the calibrated
threshold (~5 s each) and reports the spread. A **universal jog / go-to / STOP**
panel is available at the bottom of every wizard step for moving the stage away or to
a specific angle.

### Fast home + backlash (`O`) — the production-candidate routine

`O` is the full production-equivalent calibration in one pass — the complete
functionality of PortalFW's `measureBacklashRoutine` + `homeRoutine`: it finds
the **exact home centre** (midpoint of both dip edges, latched in one
forward-engaged pass at µstep resolution), the **switch size**, and the
**gear backlash** (engage-vs-release at the trailing edge), and it
**self-calibrates the comparator threshold** from the live background
(`T = background − 10`; the profile drifts 15–25 duty counts day-to-day so no
fixed threshold survives). Phases: calibrate (cold only, cached) → ramped
seek (24 k µsteps/s; the motor stalls above ~30 k at 100 k/s² accel) →
validation gates (depth / shoulder / width, so a false feature can't be
adopted as home) → precise two-edge pass (debounced ISR latch, M=32
consecutive µsteps, immune to flank-dither blips) → backlash → park + zero.

The precise measurement runs **two averaged forward passes** at 2000 µsteps/s
(a third tie-breaks if they disagree >12 µsteps): repeatability is
**σ ≈ 5 µsteps with every home within 0.03°** (worst 14 µsteps over 86
consecutive homes; ≈0.011° at thermal equilibrium). A width servo trims the
cached threshold ±1 count when the flag width drifts >45 µsteps from its
calibration anchor (thermo-optical drift tracking). Typical timings on this
rig: **~11 s warm** (threshold cached; single-pass ~7 s at σ 7.4), **~21 s
cold** including calibration and a worst-case full-rev seek. Failure always
ends with `O,done,0,...,"reason"` and clears the threshold cache.
µsteps/rev is the exact rational **189,704** (the truncated 189,696 is 7.9
short — confirmed by rotation tests, residual +3.9 ± 7).

The headless harness `monitor/bench_harness.py` drives the experiment suite
(census / knee / matrix / backlash — results land in `reports/`), and
`portalfw_port/` contains the PortalFW-ready implementation
(`FastHomeRoutine.cpp` + `PORTING.md`).

**`monitor/fast_home_gui.py`** is a focused GUI for this routine: a ring-dial
visualisation of the prism motion (needle + fading trail, flag arc and home
marker once homed, edge markers appearing live during the run), a **HOME NOW**
button with live phase readout, a jog panel (spring-back slider, ±0.1/1/10°
steps, go-to-degrees, STOP), and a **report card** after every run (home
shift, switch size, backlash, threshold/calibration, per-phase timings) plus a
run history with the accumulated repeatability σ.

```sh
monitor/.venv/bin/python monitor/fast_home_gui.py    # --port … --baud …
```
