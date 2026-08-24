# RouterRS

A Rust reimplementation of the KC79 Reworld **Router** app (the C++
openFrameworks app in `../Router`), feature-compatible with the original
plus new connection-diagnostics and session-reporting facilities. Session
reports are viewed with the companion Node.js app in `../RouterReports`.

## Running

The GUI is an **av-frameworks operator app**: a native control window (WKWebView on
macOS, WebView2 on Windows) showing a web control page served by the app itself on
`http://127.0.0.1:8780`, alongside the legacy REST (:8080) and OSC (:4000) servers.

```
# Build the web page first (its bundle is served by the binary), then the app
cd web && npm install && npm run build && cd ..
cargo build -p router-operator

# Native window + http://127.0.0.1:8780 (loads ../../config.json — cwd-independent)
./target/debug/router

# Same page, no window (open it in any browser)
./target/debug/router --headless

# Against the in-process firmware simulator (no hardware)
./target/debug/router --simulate --sim-dead 2,7 --sim-noisy 5

# Headless runtime (REST + OSC + reporting only; for soak tests / CI / bench)
cargo run -p router-headless -- --config config.json --simulate --poll 3 --duration 60
```

Common flags: `--config <path>` (also `ROUTER_CONFIG`), `--report-dir <dir>` (also
`ROUTER_REPORTS`), `--verbose` (raw packet logging), `--port <n>` (insist on the HTTP
port), `--simulate` with `--sim-dead <ids>`, `--sim-noisy <ids>`, `--sim-drop <0..1>`,
`--sim-corrupt <0..1>`.

Agents drive the same installation over `/api/router/*` on the app's HTTP port:
`state`, `diagnostics`, `logs`, `ports`, `firmware` (list/upload/flash/erase/run),
`files`, and a typed `POST /api/router/command` — everything converges on the same
command queue as the page and the legacy servers.

**Build prerequisites**: Node ≥ 22.13, Rust 1.96 (pinned by `rust-toolchain.toml`),
and the CEF headers the framework's shim compiles against
(`node third_party/av-frameworks/tools/fetch-cef.mjs`, cached per machine — needed to
*build*, not to run; the control window itself uses the system webview). The framework
is reached through the `third_party/av-frameworks` symlink into PortalFlasher's
submodule and pinned by `framework.lock`. Build the web bundle before cargo, and run
cargo from this directory (`.cargo/config.toml` carries macOS link flags).

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
- A Diagnostics panel in the GUI: KPI tiles, an installation health heatmap,
  connection table, worst-units list, live fault feed, verbose toggle,
  operator markers, on-demand summary.
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
| `router-operator` | the `router` binary: av-frameworks operator app (schema, bridge, `/api/router/*`) |

The control page lives in `web/` (React 19 + Vite, `@auroravision/av-gui`); its
tests run with `npm test`. The schema/bridge design follows PortalTestBench's
idioms: action counters, desired/observed splits, one bridge thread between the
bus and the runtime actor, documents over HTTP. Dynamic structure (rebuild
columns, add/remove sources) re-seals the schema at runtime.

`cargo test --workspace` runs protocol goldens, kinematics goldens, config
round-trips, collation property tests, and full-stack integration tests
against the simulator.

## Deliberate deltas from the C++ GUI

- RS485 debug print toggles (Print Tx/Rx/broken msgpack/ACK time) are superseded
  by the NDJSON reporter's verbose mode.
- OSC/REST enable + port are config-file settings (`config.json` "Receiver" /
  "Server"), shown as observed facts in the GUI.
- MotionControl measure settings and MotorDriver testTimer count/period ride the
  firmware defaults (as in the iced GUI); the routines themselves are exposed.
- Per-source preview thumbnails are replaced by the live composited preview.
- File selection happens through server-side listings plus browser upload
  (a webview file picker yields content, not paths): firmware `.bin`s from
  `firmware/`/`ROUTER_FIRMWARE`/the per-user upload store, videos from
  `media/`/`ROUTER_MEDIA`.
