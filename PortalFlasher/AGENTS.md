# Working in PortalFlasher

This directory is an AV `operator-app`. Its machine-readable contract is [`av-app.toml`](av-app.toml).

Before changing architecture or UI, read these completely, in the pinned framework checkout:

- `third_party/av-frameworks/AGENTS.md`
- `third_party/av-frameworks/docs/application-contract.md`
- `third_party/av-frameworks/docs/patterns.md`
- `third_party/av-frameworks/docs/constraints.md`

Primary references for this product:

| | |
|---|---|
| `examples/example-console` | the lifecycle and the standard host/shell split; `main` is one line |
| `web/src/examples/router-status.tsx` | verdict first, evidence collapsed underneath — this page's layout rule |
| `gallery` | desired and observed state drawn as two separate things on every row |
| `web/src/calibration/sounds.ts` | `SystemSounds`, and the `brand/system-sounds/*.wav` this rig depends on |

## Product outcome

A production rig for bringing up **virgin** KC79 Portal boards (STM32G070RBT6) over SWD. Armed
and hands-free: the operator seats a board on pogo pins, hears a tone, power-cycles it, hears a
second tone, and moves on **without looking at a screen**.

The required operator workflows are:

1. Select or build a device image, and see what it is (source, build id, hashes).
2. Arm — which requires an empty fixture and sounds a cue.
3. Seat a board: debounce, flash both regions, verify by full readback, "flashed, cycle it".
4. Re-seat it: run-check without halting or resetting, final pass tone.
5. Observe failures, the fault count, and the session log.

The safety boundary:

- **A lift mid-write is the worst failure this rig has.** The busy cue is a held level for the
  whole of a pass, a partial write is never resumed, and a failed board re-flashes on
  reinsertion rather than being run-checked.
- **Sound lives in the browser.** `--headless` therefore has no tones, so the rig must not stay
  armed with nobody watching: the page re-asserts `/arm/heartbeat` and the worker disarms if it
  goes stale. Closing the tab disarms the rig.
- Desired and observed state are always drawn separately (`/arm/desired` vs `/arm/observed`).
- Arming a rig with a board already in the fixture must not flash it.

## Where the work actually lives

`crates/portal-swd` is the whole of the rig's behaviour and has **no `av-*` dependency**. The
state machine there is pure and clock-injected, so every bounce case, removal-gate case and
failure case is a unit test rather than a bench session. Keep it that way: policy belongs in
`portal-swd`, and `crates/portal-flasher` should stay a schema, a worker, and a page.

If you change the state machine, run the mutation check as well as the tests — delete the
removal gate, or make a failure keep the same pass, and confirm the suite goes red. A green
suite over a machine nothing constrains is the failure mode this design is most exposed to.

## Definition of done

A successful compilation, HTTP 200, host process, or bootstrap page is not product completion.
Before calling this application operational, provide:

- a production web build and locked Rust build from a clean checkout;
- a live screenshot of the flagless native window;
- a live screenshot of the same UI in a browser;
- a `--headless` check proving no native window appears;
- evidence for every `required_surfaces` entry and workflow above;
- the bench evidence in the plan's verification section — in particular the **lift mid-write**,
  **bounce**, **double-insertion** and **close-the-tab-disarms** checks, which are the ones that
  distinguish this from a program-a-board button;
- a passing `powershell -File third_party\av-frameworks\tools\check-av-app.ps1 -AppPath .`

## Non-negotiable implementation rules

- Flagless launch opens a native window and serves the same loopback page; `--headless` opts out.
- Keep the workspace-owned `av-gui-subprocess` helper; dependency binaries are not built
  automatically, and without it the shell reports `SubprocessMissing`.
- Use `@auroravision/av-gui` runtime, controls, tokens and base styles; do not copy them.
- Declare control metadata in the Rust schema and bind controls by path.
- Never branch UI implementation on native/remote transport.
- `web/vite.config.ts` **must** set `publicDir` to the framework's `brand/` directory. The
  pass/fail WAVs are served from there and from nowhere else; without it the tones are silent
  and nothing fails visibly.

## Never run `cargo fmt --all` or `cargo clippy --fix` unscoped here

Both reach **into the pinned framework submodule** and rewrite it. `crates/av-gui-subprocess`
path-depends on `third_party/av-frameworks/crates/av-gui-cef-sys`, and those tools apply
machine-applicable fixes to every locally-pathed crate they can see — not just workspace members.
It has happened twice, each time silently reformatting 62 files of `av-frameworks`. The gitlink is
unaffected, so nothing wrong reaches a commit, but the submodule ends up dirty and every later
`git status` is noise.

Name the packages instead:

```powershell
cargo fmt -p portal-swd -p portal-flasher -p av-gui-subprocess
cargo clippy --fix -p portal-swd -p portal-flasher --all-targets
```

The same hazard applies to a drifting working directory: `cargo build --manifest-path Cargo.toml`
resolves `Cargo.toml` **relative to the cwd**, so if the shell has wandered into
`third_party/av-frameworks` that command builds the framework's own workspace instead. Use an
absolute `--manifest-path`, and if the submodule does end up dirty, restore it with
`git -C third_party/av-frameworks checkout -- .` before committing anything.

## Two things about this checkout

- **`vendor/cef` is a junction** to `C:\dev\av-frameworks\vendor\cef`, to avoid a second 420 MB
  copy. `tools/bootstrap.ps1` checks the version against `cef.lock` and fails if they diverge.
  On a machine without that sibling clone, delete the junction and let bootstrap fetch it.
- **The repository root has a pre-existing broken submodule entry**: `fonts` is committed as a
  gitlink with no `.gitmodules` record, so `git submodule update --init --recursive` at the root
  errors. It predates this application and `tools/bootstrap.ps1` works around it by naming this
  submodule explicitly. Fixing it properly is a root-repository decision, not this app's.
