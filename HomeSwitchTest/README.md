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

Holds the threshold **fixed** at `HOMESWITCHOPTICAL_DEFAULT_THRESHOLD` (178) and
reads both comparator outputs as fast as possible — no sweeping, so it tracks the
live home-switch state in realtime.

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
