# KC79 Reworld

Firmware, tooling and applications for the KC79 portal modules: an STM32G070RBT6 board that moves
two prisms, talks a MessagePack-over-COBS protocol on RS485, and is field-updatable through its own
bootloader.

This file is about **building things** — the firmware, the bench, and the package that carries both.
For the wire protocol, V1/V2/V3 topology, and repeater commissioning read
[`Protocol.md`](Protocol.md); for what each project *is*, read its own README. Repeater-specific
build/recovery instructions and field evidence live in [`RS485Repeater/`](RS485Repeater/README.md).

```
node tools/build-firmware.mjs        # PCB v6 + v4 applications, and the bootloader
~/.platformio/penv/bin/pio run -d RS485Repeater  # ESP32-C3 RS485 repeater
node PortalTestBench/tools/build.mjs # the bench that flashes them
node tools/package.mjs               # both, wrapped up for a machine that has neither
```

Every one of those runs on Windows and macOS. In VS Code they are tasks; **Ctrl/Cmd+Shift+B**
builds the bench, and **Package: distributable** does the whole chain in the one order that works.

---

## What is here

| | |
|---|---|
| [`PortalFW/`](PortalFW) | The application firmware. PlatformIO + Arduino, links at `0x08006000` |
| [`PortalBootloader/`](PortalBootloader/README.md) | The RS485 field-update bootloader. PlatformIO + STM32Cube, `0x08000000`, 16 kB. Also holds `include/portal_flash_layout.h`, the flash map every project here reads |
| [`RS485Repeater/`](RS485Repeater/README.md) | The Reworld V3 ESP32-C3 frame router between the shared outer bus and a nine-Portal branch. PlatformIO + Arduino |
| [`PortalTestBench/`](PortalTestBench/AGENTS.md) | One module on a bench: flash it, drive it, watch it, record it. Rust + a React page |
| [`PortalFlasher/`](PortalFlasher/AGENTS.md) | The production SWD rig for virgin boards. Shares `portal-swd` with the bench |
| [`RouterRS/`](RouterRS/README.md) | The host-side router (Rust). [`Router/`](Router) is the C++ original |
| [`RouterReports/`](RouterReports/README.md) | A viewer for the NDJSON sessions the router writes |
| [`HomeSwitchTest/`](HomeSwitchTest/README.md) | Optical home-switch bench tooling, outside PortalFW |
| [`tools/`](tools) | The cross-platform build and packaging scripts this file describes |

Three separate Cargo workspaces, three PlatformIO projects, one MSVC solution. Nothing builds them
all at once, and nothing should.

---

## Prerequisites

Both platforms need **Rust 1.96** (pinned in `PortalTestBench/rust-toolchain.toml`), **Node 22.13+**
(`.node-version`), and **PlatformIO** — only for building firmware, never for flashing it.

`pio` is on nobody's `PATH` by default. `tools/build-firmware.mjs` looks there first and then in
`~/.platformio/penv/`, which is where the installer actually puts it.

### macOS, additionally

```sh
node PortalTestBench/tools/bootstrap.mjs   # links the framework, npm ci, and fetches CEF
```

**CEF is a build prerequisite here, and only that.** `av-operator-app` depends on `av-gui-shell`
unconditionally, so `av-gui-cef-sys` compiles on every target and its build script panics without
`vendor/cef`. About 124 MB downloaded, ~574 MB unpacked, once, into a cache shared by every
checkout on the machine. **Nothing loads it at run time**: this application opens a control window
on WKWebView, and `vmmap` on a live process reports zero CEF images. No Vulkan SDK is needed at
all, for building or for running.

That is the opposite way round on Windows, and the asymmetry matters before you trim a package:
there `av-gui-cef-sys` links `libcef` as an import library, so `libcef.dll` is resolved through the
executable's import table *before `main` runs* whether anything calls it or not.

### Windows, additionally

MSVC, and the **WebView2 Runtime** at run time (Windows 11 ships it; a Windows 10 image may not).
The bench declares a control window on both platforms — WebView2 here, WKWebView on macOS — which
is the lightest kind that carries it: no compositor, no CEF payload, no helper process. Measured
idle difference against a composed window on this framework: roughly 8% of one core and 396 MB
against 19–30% and 524 MB.

`powershell -File PortalTestBench\tools\bootstrap.ps1` still works and always will; it is three
lines that call the same `.mjs`.

---

## Building firmware

```sh
node tools/build-firmware.mjs                            # all three
node tools/build-firmware.mjs --env application_bank_optical   # PCB v6 alone
node tools/build-firmware.mjs --list                     # what exists, and what is refused
node tools/build-firmware.mjs --clean                    # rebuild from nothing
```

| Environment | PCB | Loads at | |
|---|---|---|---|
| `application_bank_optical` | v6 | `0x08004000` | optical home switch — **the production default** |
| `application_bank_mechanical` | v4 | `0x08004000` | rev-1 mechanical switches, `-D HOME_SWITCH_LEGACY` |
| `application_bank_optical_legacy_base` | v6 | `0x08006000` | for a board still on bootloader v4/v5 — see below |
| `application_bank_mechanical_legacy_base` | v4 | `0x08006000` | as above |
| `bootloader` | — | `0x08000000` | RS485 field update, 16 kB bank |

**Two application bases, and both are current.** Bootloader v6 is 16 kB where v4/v5 were 24 kB, so
the application moved down to `0x08004000` and gained 8 kB. A board still carrying an old bootloader
runs its application at `0x08006000`, and a v6 bootloader will also start an image there when the
new bank is blank — which is the state a board is in between having its bootloader replaced and its
application re-uploaded.

An image says which bank it was built for, in a descriptor at `base + 0xC0`. The bootloader refuses
to *start* one whose descriptor disagrees with where it sits, and the flasher refuses to *send* one
to a board that would not run it — the two images are otherwise indistinguishable, and starting the
wrong one hard-faults later at an unrelated address. The `*_legacy_base` builds drop the OLED
(`GUI_DISABLED`) because the older bank is 8 kB smaller and they do not otherwise fit.

Output lands at `<project>/.pio/build/<env>/firmware.bin`, which is where the bench looks for it.
There is no merge step and there never will be: the RS485 field-update path can only ever ship an
application image starting at offset zero, so a combined image would be useless to the one path that
most needs it.

**Three environments are refused by name.** `no_bootloader` and `debug_no_bootloader` link at
`0x08000000`, so they program cleanly into the *application* slot, verify cleanly, and never run —
the mistake that costs a bench session. `application_bank_optical_bringup` suppresses
`Routines::startup()`, so a board flashed with it never homes on its own. Run
`--list` to see them and the reason for each.

Every image is checked before the script reports success: its size against its bank, its reset
vector against that bank, and its initial stack pointer against this part's 36 kB of SRAM. That is
the same pair of refusals `portal-swd` applies at flash time, moved earlier — one check is a policy
and two are a guarantee.

### The PlatformIO version is pinned, deliberately

Both `platformio.ini` files pin `platform = ststm32@<version>`. Unpinned, PlatformIO resolves
whatever release a given machine already carries and the compiler comes with it — so two machines
produce two different binaries from one commit, and the git sha a package records says nothing about
which. It is not hypothetical: unpinned, PortalBootloader compiled on Windows and failed on macOS,
because the older `framework-stm32cubeg0` there takes a non-`const` `uint8_t *` in
`HAL_UART_Transmit`.

Raising a pin is a one-line change plus a rebuild, a size check, and a board.

---

## Building the bench

```sh
node PortalTestBench/tools/bootstrap.mjs     # once
node PortalTestBench/tools/build.mjs         # --release for a package
node PortalTestBench/tools/test.mjs          # --fast for gates 1-3
```

**The web bundle is built before cargo, and never in parallel.** `av_operator_app::web_assets!`
resolves `web/dist` at *compile* time, so a cargo build that runs first embeds whatever the last web
build left behind — a binary that starts cleanly and serves a stale page, which reads as a host bug
rather than a missing build step.

Every cargo call passes an absolute `--manifest-path`. There is a second complete Cargo workspace
behind `third_party/av-frameworks`, and a shell that has wandered in there builds the framework
instead, successfully, with nothing saying so.

One gate is Windows-only: `check-av-app.ps1` is PowerShell and belongs to the pinned framework
submodule. `test.mjs` announces the skip on macOS rather than passing quietly — run it on Windows
before releasing a package.

### Running it

```
portal-test-bench              a window, and http://127.0.0.1:8770
portal-test-bench --headless   the same page, no window
portal-test-bench --simulate   a modelled module: no probe, no port, no board
ptb state                      the same bench, for an agent or a script
```

On macOS the bare binary opens its window perfectly well — this shell loads no CEF framework and
launches no helper processes, so there is nothing for a bundle to resolve. Bundle it when you want
the application's own Dock icon, menu-bar name and signature rather than the terminal's:

```sh
node PortalTestBench/tools/bundle-macos.mjs --profile debug
open PortalTestBench/target/debug/bundle/PortalTestBench.app
```

---

## Packaging

```sh
node tools/package.mjs                       # builds everything, then wraps it
node tools/package.mjs --skip-build          # use what is already in target/ and .pio/
node tools/package.mjs --sign "Developer ID Application: ..."
```

Produces `dist/PortalTestBench-<sha>-<platform>-<arch>.zip`: the bench, the `ptb` CLI, the test
plans, and prebuilt firmware for both PCB revisions plus the bootloader. **It needs no repository,
no Rust, no Node and no PlatformIO on the far machine.**

```
macOS                                     Windows
  PortalTestBench.app/Contents/             portal-test-bench.exe
    MacOS/  portal-test-bench               ptb.exe
            ptb                             libcef.dll + payload, locales/
    Resources/  plans/  firmware/           resources/  plans/  firmware/
  README.txt                                README.txt
```

The macOS side is about 20 MB and the Windows side is not, for the import-table reason above:
`libcef.dll` has to travel there even though nothing ever calls it.

The payload sits beside the executable on Windows and in `Contents/Resources` inside a bundle,
because `Contents/MacOS` is for executables and every macOS convention expects data in
`Resources`. `portal_swd::artefacts::resource_roots` knows both layouts and neither is gated on
the host OS — a `.app` is a directory and a zip is a directory, and a developer who unpacks one on
the other platform to look inside should get the same answer the operator gets.

`firmware/` mirrors the shape of a built repository —
`PortalFW/.pio/build/<env>/firmware.bin` — so `portal_swd::artefacts::discover_in` serves a package
and a developer's tree with one implementation, and there is no second discovery path to keep in
agreement. `MANIFEST.md` beside it records each image's environment, load address, size, PlatformIO
pin and sha256; nothing reads it, and it is the only place a `.bin` can say what it is.

**A dirty tree is refused** unless you pass `--allow-dirty`. `PortalFW/set_build_date.py` compiles
the same git description into `Version.h` and therefore into the firmware, so a package whose
manifest names one commit and whose firmware reports another is worse than no manifest at all.

### Where a packaged copy looks for things

A binary built here bakes `CARGO_MANIFEST_DIR` in at compile time — right for a developer, and
meaningless on a machine that has never held this repository. Three paths therefore layer their
answers, preferring an explicit request over what was shipped over what was compiled in:

| | Override | Then | Then |
|---|---|---|---|
| firmware | `PORTAL_FIRMWARE_DIR` | `<resources>/firmware` | the repository |
| plans | `PORTAL_TEST_BENCH_PLANS` | `<resources>/plans` | `PortalTestBench/plans` |
| sessions | `PORTAL_TEST_BENCH_REPORTS` | the per-user state directory | `PortalTestBench/reports` |

Sessions are the one that differs, and deliberately: a `.app` in `/Applications` and a zip unpacked
into `Program Files` are both read-only to the operator running them, and the session `.ndjson` is
this product's evidence — the one file that must not be the thing that fails. A packaged run writes
to `~/Library/Application Support/AuroraVision/av-frameworks/portal-test-bench` or
`%LOCALAPPDATA%\AuroraVision\av-frameworks\portal-test-bench`.

### Signing

The macOS bundle is signed **ad hoc** by default, which is enough for Gatekeeper to let it open
after one right-click → Open, and not enough to open on a double-click. `--sign <identity>` takes a
real Developer ID; notarisation is a further step this script does not do. `README.txt` in the
package tells the recipient which of those they are holding.

There are no nested bundles to sign — no CEF framework, no helper apps — so signing is one call
followed by `codesign --verify --deep --strict`, which is a hard gate: `codesign` succeeding says
the signature was *written*, not that it is valid.

---

## Verifying a package

The gates that can be run anywhere are `node PortalTestBench/tools/test.mjs`. These are the ones
that cannot:

1. Unpack it on a machine with **no repository, no Rust, no Node, no PlatformIO**, and open it.
2. `curl http://127.0.0.1:8770/api/bench/firmware` — `missing` must be empty, `root` must be inside
   the package, and all four artefacts must read `fits: true`.
3. Look at the window. It is the one check nothing automated here replaces.
4. **Flash a board with it** — bootloader plus the v6 application, chip-erase, full 128 kB readback
   verify, and the boot check reaching the running application. That is the only test that covers
   every path the package touched.

---

## A note on documentation

`.gitignore` ignores `docs/` **at any depth**. A new document has to be a `README.md` (or named
something else entirely) or it is silently untracked, and nobody finds out until the clone.
