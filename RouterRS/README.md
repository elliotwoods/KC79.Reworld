# RouterRS

A Rust reimplementation of the KC79 Reworld **Router** app (the C++
openFrameworks app in `../Router`), feature-compatible with the original
plus new connection-diagnostics and session-reporting facilities. Session
reports are viewed with the companion Node.js app in `../RouterReports`.

## Running

```
# GUI (loads ./config.json like the C++ app; same schema)
cargo run -p router-app

# GUI against the in-process firmware simulator (no hardware)
cargo run -p router-app -- --simulate --sim-dead 2,7 --sim-noisy 5

# Headless runtime (REST + OSC + reporting; for soak tests / CI / bench)
cargo run -p router-headless -- --config config.json --simulate --poll 3 --duration 60
```

Common flags: `--config <path>`, `--report-dir <dir>`, `--verbose` (raw
packet logging), `--simulate` with `--sim-dead <ids>`, `--sim-noisy <ids>`,
`--sim-drop <0..1>`, `--sim-corrupt <0..1>`.

## Feature parity with the C++ Router

- **Wire protocol**: COBS-framed MessagePack, envelope `[target, source,
  body]`, byte-compatible encoders (both the msgpack11 minimal-int path and
  the msgpack-c forced-int8 path), 300 ms source-ID ACK windows, broadcast
  gaps, outbox collation (latest per address/target), 20 s reconnect.
  Golden-tested against a captured wire frame and MSVC-compiled oracles.
- **Pilot kinematics**: bit-exact port (f32/f64 promotions preserved),
  verified against 6,900+ MSVC-generated golden vectors
  (`tests-fixtures/pilot_oracle.cpp` / `pilot-vectors.csv`). Known C++
  quirks are kept deliberately and marked `BUG-COMPAT` in the source
  (`findClosestAxesCycle` axis-A distance, `unwind()` reading polar,
  `seeThrough()` method vs action asymmetry, transmit enum spelled
  `"Inidividual"` in configs).
- **Model**: Installation → Columns (one RS485/TCP bus each) → Portals
  (target IDs 1..N) with Pilot, per-axis MotionControl/MotorDriver, motor
  driver settings, firmware log history; all 12 broadcastable actions;
  Individual / Keyframe (batched, optional velocities) / Disabled transmit.
- **Servers**: OSC receive-only on UDP 4000 (all `Routes.cpp` routes incl.
  `/axesMoveByInidices` — sic) and REST on HTTP 8080 (same GET routes and
  status codes as the crow server).
- **Image pipeline**: Gradient / Text / FilePlayer (via `ffmpeg` on PATH;
  whole file decoded to the installation resolution in memory for exact
  Loop/PingPong/None + speed + position control) / Spout. Composite styles
  Direct / HV_ThetaR / Centered ported verbatim (including the C++'s
  alpha and row-sampling quirks, marked BUG-COMPAT).
- **Firmware update**: single-column and mass update ("FW"/"ER"/"RU" magic
  words, 32-byte frames with the XOR-16 checksum, original pacing).
- **Config**: loads the same `config.json` (exact ofParameter names or
  lowercased-first-word keys, `columnCommonSettings` merge with nlohmann
  `update` semantics). NEW: `save` writes it back (atomic, preserves
  unknown keys, emits keys the C++ app also accepts).

**Known gap**: the Spout receiver requires a small SpoutDX C shim compiled
against the Spout SDK (documented in
`crates/router-core/src/image/sources/spout.rs`) and the `spout` cargo
feature; without it the source loads but renders black with a status note.

## New diagnostics (beyond the C++ app)

- Per-column connection stats (tx/rx/timeouts/decode errors/latency
  percentiles), per-portal health scoring (ACK rate, latency, firmware
  error-log rate, silence-while-polled, calibration flags) with an
  ok/degraded/faulty/silent state machine and hysteresis.
- A Diagnostics panel in the GUI: connection table, worst-units list, live
  fault feed, verbose toggle, operator markers, on-demand summary.
- NDJSON session logs + JSON summaries in `reports/` (schema:
  `docs/report-schema.md`), consumed by `../RouterReports`.
- An in-process firmware simulator (`--simulate`) with fault injection
  (dead units, noisy loggers, reply drops, line corruption) for development
  and bench-testing without hardware.

## Workspace

| crate | contents |
|---|---|
| `router-proto` | wire protocol: COBS, envelopes, command builders, reply parsing, FW frames (golden tests) |
| `router-core` | model, kinematics, RS485 workers, simulator, config, image pipeline, OSC/REST servers, runtime |
| `router-report` | NDJSON reporter, aggregation, health scoring, summary builder |
| `router-headless` | CLI runtime |
| `router-app` | iced GUI |

`cargo test --workspace` runs protocol goldens, kinematics goldens, config
round-trips, collation property tests, and full-stack integration tests
against the simulator.
