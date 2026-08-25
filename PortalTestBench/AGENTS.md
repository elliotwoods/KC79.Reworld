# PortalTestBench — working notes

The bench instrument for a **single portal module**: flash it, connect to it, drive it, watch
it, and produce evidence. It is an **operator-app** on the Rust `av-frameworks` stack, and it
is designed to be driven by a human at the GUI and by an agent from the command line *at the
same time*, on the same hardware, through the same command queue.

## Read these first

Before changing anything here, read the framework's own contract — this app is downstream of
it and inherits its rules:

- `third_party/av-frameworks/AGENTS.md`
- `third_party/av-frameworks/docs/application-contract.md` — normative; §1a UI kinds, §2
  process rules, §4 the status bar, §6 the acceptance gates
- `third_party/av-frameworks/docs/patterns.md` — §1 schema-first, §2a window kind, §9 status
  bar, §12 hardware-is-a-provider, §15 long work
- `third_party/av-frameworks/docs/constraints.md` — six non-negotiable UI rules

`PortalFlasher/` in this repository is the reference downstream app and the closest thing to a
worked example; copy its idioms rather than inventing new ones.

## The five things that will catch you here

1. **`third_party/av-frameworks` is a LINK, not a submodule** — a junction on Windows, a symlink on
   macOS. It points at `PortalFlasher/third_party/av-frameworks`. One checkout, one pinned
   revision, shared by both apps. Consequences: never run an unscoped `cargo fmt --all`, `cargo clippy --fix` or
   `cargo fix` — they reach through the junction and rewrite the framework, which dirties
   **PortalFlasher's submodule** as well as this app. Always name packages:
   `cargo clippy -p bench-core -p portal-test-bench -p ptb -p av-gui-subprocess`. `tools/test.mjs`
   gate 7 fails the build if the checkout is dirty. `framework.lock` records the revision this app
   was bootstrapped against; bootstrap warns when PortalFlasher moves it.

2. **Always pass an absolute `--manifest-path`.** There is a second complete Cargo workspace
   behind that junction. A shell that has wandered into it builds the framework instead —
   successfully, with nothing saying so.

3. **Web bundle first, then cargo, never in parallel.** `web_assets!` resolves `web/dist` at
   compile time. A Rust-first build embeds the previous bundle and serves a stale page, which
   reads as a host bug rather than a missing build step. `tools/build.mjs` enforces the order.

4. **This app is `ui = "control-window"` and must NOT have an `av-gui-subprocess` crate.**
   Nothing is drawn underneath the page — the plots are canvas in the DOM. The CEF helper is a
   `composed-window` requirement; `check-av-app.ps1` warns (AVAPP115) if a control-window app
   ships one. PortalFlasher has one only because its manifest omits `ui`.

   This was briefly `composed-window`, and the reason is worth carrying because it looked like a
   platform fact and was not. `av-gui-webview` declared `tao`/`wry` under `cfg(windows)` alone, so
   `av-operator-app` answered `NativeUnavailable` for `control-window` off Windows and a macOS
   window meant declaring the heavier kind — a CEF payload, four helper bundles and a Vulkan
   compositor, to draw a page that needed none of them. tao and wry cover WKWebView; the crate had
   simply never been built there. Porting it was five `cfg` gates. **Before accepting a platform
   limit, check whether it is a limit or an untried path.**

5. **CEF is a build prerequisite on macOS and a run-time one on Windows.** `av-operator-app`
   depends on `av-gui-shell` unconditionally, so `av-gui-cef-sys` compiles everywhere and its build
   script panics without `vendor/cef`; `tools/bootstrap.mjs` fetches it. Nothing loads it at run
   time here — `vmmap` on a live macOS process reports zero CEF images — but on Windows `libcef`
   is an *import library*, so `libcef.dll` is resolved through the import table before `main` runs
   whether anything calls it or not. That is why the Windows package carries the payload and the
   macOS bundle does not.

   `PortalTestBench/.cargo/config.toml` carries the `-Wl,-U,_cef_*` allowances, regenerated with
   `nm -u` over the shim objects rather than copied: cargo config does not inherit across
   workspaces, and the framework's own list has been out of step before. They are still needed —
   the shim's objects reach the link line even though nothing calls into them.

   And the trap that cost a release build: **cargo reads `.cargo/config.toml` from the working
   directory upwards, not from `--manifest-path`.** Set both, always. A build invoked from the
   repository root silently drops every flag in that file and fails at link.

## Where behaviour lives

`crates/bench-core` is the whole of the bench's behaviour and has **no `av-*` dependency** —
the same rule PortalFlasher states for `portal-swd`. Transports, the test engine, verdicts,
threshold calibration, flashing policy and the report vocabulary all live there, and its whole
suite runs with no probe, no serial port, no board and no browser. `crates/portal-test-bench`
should stay a schema, a worker, some routes and a page. `crates/ptb` is the CLI and depends on
`bench-core` directly, which is why `ptb --local` can own the hardware with no GUI at all.

## Two rules specific to this product

**Only one thing may poll the link.** The firmware's `logger` field **drains its outbox when
read** — log lines are delivered exactly once, to whoever polled. Two pollers means each sees
roughly half the log, and neither looks obviously wrong. So `Link` is neither `Clone` nor
`Sync`, it is moved into the worker thread at `start()`, and `Link::poll` is called from
exactly one place in the tick. Every consumer — GUI parameters, telemetry rings,
`/api/bench/log`, the NDJSON writer — reads a fan-out mirror. **HTTP handlers never touch
hardware**; they enqueue a command and read the mirror.

**The dead-man is inverted relative to PortalFlasher.** There, a stale `/ui/heartbeat` disarms
the rig, because an unattended flasher with a pogo fixture is dangerous. Here, a stale
heartbeat blocks *starting* destructive work but **never cancels a run in flight** — closing
the browser tab must not abort an eight-hour soak. That would be a worse failure than a silent
one. This is deliberate; it will read as a bug to anyone arriving from the flasher.

## Hardware truths the engine encodes

These are measured, they survive firmware rewrites, and each is enforced by
`Plan::validate()` with a unit test that fails if the invariant is deleted:

- **The optical home threshold must be self-calibrated per run.** The production
  injection-moulded ring's background is *unmeasurable* (it never crosses at any threshold
  0–255), so the old `T = background − k` rule is structurally impossible. The rule is
  band-centred: `T_op = T_floor + 0.55·band`. A plan that homes an optical module without a
  preceding `CalibrateThreshold` is rejected **before the run starts**. `T_floor`, `band` and
  `T_op` are recorded on every run — a home result whose threshold is not in the report is not
  evidence.
- **The stall cliff is real**: ~30–32k µsteps/s at 32:1. Moves above `stall_guard` are refused
  outside a `characterise` plan, whose whole job is to find the cliff.
- **150 mA fixed + driver auto-sleep**; current between 100–250 mA is indistinguishable in slip
  and in homing precision, so there are no dynamic current games.
- **µsteps/rev is 189,704 at 32:1 and 92,252 at 16:1** (both measured; the nominal figures are
  wrong). The firmware auto-detects the ratio; never assume one.
- **16:1 modules are thermally limited** to roughly 23–27°/s sustained. 32:1 sustains ≥60°/s
  and is the production gearing.

## Before claiming it works

The framework's acceptance gates apply (`application-contract.md` §6) and are not optional:

- `node tools/test.mjs` — all gates, including the simulated end-to-end run. Gate 5
  (`check-av-app.ps1`) is PowerShell and part of the pinned submodule, so it **skips on macOS and
  says so**: run it on Windows before a release. `tools/{bootstrap,build,test}.ps1` still exist and
  are three-line wrappers around the same `.mjs`
- **Screenshots**: the flagless native window on both shells — on Windows a frameless WebView2
  window, including the page's own caption strip actually dragging and all three window buttons
  working; on macOS a decorated WKWebView window, where AppKit draws the traffic lights and the
  page draws no caption strip at all — plus the same UI in a browser, and `--headless` opening no
  window
- **For a package**: the checks in the repository [`README.md`](../README.md) under "Verifying a
  package" — unpacked on a machine with no repository, `missing` empty in
  `/api/bench/firmware`, a `--type=renderer` in the process tree, and a real board flashed
- Evidence for each entry in `required_surfaces`
- A `ptb run --wait` transcript against a real module, and an abort mid-`Home` showing
  `escapeFromRoutine` in the NDJSON

A green suite over a machine that nothing constrains is this design's most likely failure mode.
Before a release, run the **mutation check**: make `verdict::evaluate` return `Pass`
unconditionally, or delete the threshold precondition in `Plan::validate`, and confirm the
suite goes red.
