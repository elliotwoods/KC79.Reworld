# RouterRS — agent orientation

The Router as an av-frameworks operator app: one binary (`router`) that runs the
installation model, the RS485 buses, the legacy OSC/REST servers, and serves its own
web control page on `http://127.0.0.1:8780`.

## The five things that will catch you here

1. **Build order is web first, then cargo, from this directory.** The binary serves
   `web/dist`; build it with `cd web && npm run build`. Cargo must run with this
   directory as cwd — `.cargo/config.toml` carries the macOS `-Wl,-U,_cef_*` link
   allowances and cargo reads it from the invoked workspace root, not the manifest
   path. CEF *headers* are a build prerequisite even though the control window uses
   the system webview (`node third_party/av-frameworks/tools/fetch-cef.mjs`).

2. **`third_party/av-frameworks` is a symlink** into PortalFlasher's submodule,
   pinned by `framework.lock`. Never run unscoped `cargo fmt --all` / `clippy --fix`
   — they reach through the symlink and dirty that checkout. Name packages.

3. **`router-proto`, `router-report` and `router-link` are consumed by
   PortalTestBench by path.** Their manifests stay edition-2021 and untouched;
   gate any change with `cargo check -p portal-test-bench` in `../PortalTestBench`.
   Only `crates/router-operator` is edition-2024.

4. **The bridge is the only code touching both the bus and the runtime**
   (`crates/router-operator/src/bridge.rs`). GUI actions are monotonic counters;
   desired params are diffed against last-seen mirrors (the echo-loop guard);
   telemetry writers are claim-once per schema epoch — a re-seal (rebuild columns,
   add/remove source) breaks the inner run loop and re-acquires them. The model
   thread, RS485 workers and their frozen timing constants are not touched by GUI
   work; that is the performance contract.

5. **Three front doors, one queue.** The page writes bus params, agents POST
   `/api/router/*`, integrators keep OSC :4000 and REST :8080 — all converge on the
   runtime's `Command` channel. HTTP handlers only read `Shared` mirrors or enqueue.

## Verify

```
cargo test --workspace                # protocol/kinematics goldens + integration
cd web && npm test && npm run build   # page math tests + bundle
./target/debug/router --simulate --headless   # then curl :8780 and :8080
cd ../PortalTestBench && cargo check -p portal-test-bench
```

Normative framework docs: `third_party/av-frameworks/docs/{application-contract,
operator-app-starter,patterns,constraints,traps}.md`. The reference downstream app
is `../PortalTestBench` (same idioms, same layout).
