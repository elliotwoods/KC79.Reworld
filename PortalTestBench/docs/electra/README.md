# Electra Mini manual control

Run from PortalTestBench:

```sh
node tools/build.mjs
./target/debug/portal-test-bench --simulate --port 8771
```

The bench discovers an existing av-frameworks daemon or starts the companion
`av-frameworks-daemon` beside its executable. The daemon exclusively owns the
Mini vendor-bulk interface on macOS through IOKit. `node tools/bundle-macos.mjs
--profile debug` includes the companion in the local development app bundle.
The normal GUI and browser use the same manual parameters and command queue.

## Device pages

The top knobs execute Jog −, Jog +, Home and Stop. Bottom knobs edit axis, jog
increment and speed; press bottom-right for settings. Position is large, in
revolutions, with operation feedback above it. The settings page has command
route, acceleration, minimum speed and target; top-left moves to that target,
top-right remains Stop. Button 1 returns to the main page.

Stop clears queued commands on the selected route before Escape. Motion is
refused while the fixture/test is busy or the page heartbeat is stale. Jog needs
known position and gearing; Axis B is inverted consistently with the existing
GUI. Profiles are bounded at 28,000 µsteps/s. Optical Home requires a calibrated
threshold; unknown measurements are never displayed as zero. The existing
threshold-calibration engine step is not yet wired to a transport, so that
operation cannot establish a threshold in this build.

`--simulate` models a 32:1 module at 189,704 µsteps/rev. This is fixture metadata,
not evidence measured from an attached portal. Only simulated motion was used
for this implementation's validation.

## Compatibility

EMP/1.2 adds momentary Action fields and a negotiated layout flag. Older peers
retain their existing grid and value messages; action fields are omitted when
unsupported. The operator layout is opt-in. Text values now retain bounded UTF-8
and update on screen. Activation includes revision and sequence validation;
held presses and replayed packets cannot retrigger commands. Daemon Poll replies
carry optional capabilities so firmware negotiation and reconnect can update a
running client without restarting it. Older peers ignore this additive field.

## Knob numbering

Firmware input indices now follow reading order: 0–3 across the top, 4–7 across
the bottom (the device labels these Dial 1–8). Rotation, pushes, touch masks,
calibration selection and input reports share that order. Generic pages containing
1–4 controls place them on the lower row, left-to-right; pages with 5–8 controls
start at top-left. The firmware uses one layout mapping for rendering, input,
the opposite-row editor and page indicators. There is no app-side row conversion. Stored calibration keeps its version-1
physical slot encoding and is translated at the persistence boundary, preserving
both upgrades and downgrades. The opt-in Test Bench operator layout retains its
published semantic slot assignments and its existing physical actions.

## Firmware source

The working firmware repository is `../../../.local/electra-mini-fw`, cloned from
`https://github.com/elliotwoods/electra-mini-fw` at
`3c357964514460e19ad135160c02260b59dd2d79`. It remains a separate Git repository.
`electra-mini-fw.patch` preserves all firmware/tool changes here, including new
files, so the local checkout is reproducible without an unpublished Git pointer:

```sh
git clone https://github.com/elliotwoods/electra-mini-fw
cd electra-mini-fw
git checkout 3c357964514460e19ad135160c02260b59dd2d79
git apply /path/to/PortalTestBench/docs/electra/electra-mini-fw.patch
python3 tools/host.py test
```

Build using the repository's ARM toolchain and `cmake --build build --target app`.
On macOS install Python pyserial/Pillow, then use
`python3 tools/deploy/flash_usb.py build/app.srec`. The updater uses daemon
maintenance to release USB, writes only the application bank, verifies CRC,
launches the app and resumes daemon ownership. Neither boot stage is rewritten.

## Current local session

The native development bundle is running in simulation at `http://127.0.0.1:8771`.
Its daemon administration page is `http://127.0.0.1:62109` (discovered ports change
on restart). The previous, debugger-stopped app still holds port 8770.
The development bundle is at `target/debug/bundle/PortalTestBench.app`; it uses
repository resources and is not a standalone release package.

## Validation

- 172 bench Rust tests, 70 web tests, TypeScript/Vite and scoped clippy passed.
- Framework mapper, EMP, remote-client and daemon regression suites passed,
  including changing capabilities without reconnecting.
- Portable firmware C suites passed; application image checks and on-device CRC
  verification passed (installed application CRC32 `FC4AB010`).
- Browser commands reached +1,897 µsteps on A and −1,897 on B for a 0.01 rev jog.
- Actual device screenshots and native/browser window captures are alongside this file.
- Headless simulation served HTTP successfully and created no native window.
- `node tools/test.mjs` reaches its final clean-framework gate, which fails because
  this task intentionally changes the framework submodule. It was not disabled.
  Its Windows-only contract check is skipped on macOS. Its CLI e2e gate remains
  an existing placeholder, so the explicit GUI simulation checks above provide
  the manual-flow evidence.
- Real portal motion and Windows execution remain untested. Physical encoder
  interaction still needs the operator's check; protocol press-edge behavior is
  covered by firmware and host tests.

## September 7: settings and Gallery compatibility

See [the validation record](gallery-sep7/README.md) for settings before/after,
physical Mini captures and the four-application acceptance run. The updated
firmware is installed; Gallery and the example binaries were rebuilt together.

See [top-first numbering verification](numbering-sep7/README.md) for the subsequent
firmware numbering change, calibration compatibility checks and new device captures.

See [immediate hover verification](hover-sep7/README.md) for the desktop latency
fix, softer fill, delayed-device regression and measured before/after traces.

See [small-page ergonomics](ergonomics-sep7/README.md) for the subsequent 1–4-control
lower-row policy, boundary tests and actual four/five-control device captures.
