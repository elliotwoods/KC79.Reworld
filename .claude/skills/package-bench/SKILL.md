---
name: package-bench
description: Build a self-contained PortalTestBench distribution zip for macOS — the app, the ptb CLI, the test plans, and a full firmware set, verified to run on a machine with no repository, no toolchain and no Homebrew — and optionally ship it to the repository's GitHub releases page. Use when the user asks to package, ship, release, zip, publish or "send someone" PortalTestBench, or wants a build a colleague can unzip and flash boards with.
---

# /package-bench

Two commands, in order. Do not re-derive any of this — run them, read what they printed, and
report. Everything below is already decided.

```sh
cd /Users/elliot/dev/KC79.Reworld/PortalTestBench
node tools/package.mjs --allow-dirty                        # build the zip
node tools/publish-release.mjs --allow-dirty                # draft a GitHub release from it
```

**Publishing is opt-in and outward-facing.** `publish-release.mjs` drafts by default; `--publish`
makes it live. Ask the user before passing `--publish` — a release tag is permanent in everyone's
clone the moment anybody fetches, and `gh release delete` does not un-download an asset. If they
only asked for "a package", stop after the first command.

---

## 1. Build — `tools/package.mjs`

Output: `/Users/elliot/dev/KC79.Reworld/dist/PortalTestBench-<sha>-macos-<arch>.zip` (≈10 MB). The
script prints the path on the last line.

**About 4 minutes from cold** — a release cargo build dominates. Give the Bash call a 600000 ms
timeout and let it run; do not background it and poll.

| flag | when |
|---|---|
| `--allow-dirty` | **almost always needed on this tree.** Without it the script refuses whenever `PortalFW/` or `PortalBootloader/` has uncommitted changes, and it is right to: `set_build_date.py` compiles the git description into the image, so a manifest naming a commit the firmware does not report is worse than no manifest. With it, the package records `-dirty` and the README explains what that means. |
| *(none)* | the clean-tree case. Builds the web bundle, the Rust workspace in release, and all five firmware environments, then wraps and verifies. |
| `--skip-build` | iterating on the packaging or the README wording. Uses whatever is in `target/` and `.pio/build/`. Never for something going to a person. |
| `--sign "Developer ID Application: ..."` | a real identity instead of ad hoc. Notarisation is a further step this does not do; the shipped README and the release notes adjust their own wording either way. |
| `--profile debug` | only for testing the script. The debug binary is 25 MB and slow. |
| `--skip-verify` | do not. See below. |

### What it already checks, so you do not have to

Each of these fails the build rather than warning. If it printed `Verified:` and a zip path, all of
it passed and there is nothing left to test by hand:

- **Linkage.** `otool -L` over both staged binaries, refusing anything outside `/System/Library`
  and `/usr/lib`. This is what keeps "no Homebrew, no libusb" true rather than remembered —
  probe-rs reaches USB through `nusb`, which is pure Rust, and a `cargo update` swapping it for
  `rusb` would introduce a Homebrew link that still built here and died on the far machine.
- **The firmware set.** All four `APPLICATION_ENVS`, the built bootloader and the committed
  reference image — six artefacts, each with its `.elf` beside it. The ELF is not optional:
  `Discovery::run_check_for` resolves `g_liveness_counter` from it, and without one the run-check
  cannot tell a running board from a hard-faulted one.
- **That the package works.** It unpacks the finished zip into a temporary directory with
  `PORTAL_FIRMWARE_DIR` and friends cleared, runs the bench headless, and asserts over
  `/api/bench/firmware` that `missing` is empty, that all six artefacts are listed and `fits`, and
  that `root` resolves *inside* the package. That last one is the check worth having: the failure
  it catches is a payload in the wrong place, where the app starts perfectly and offers nothing to
  flash.

---

## 2. Ship — `tools/publish-release.mjs`

```sh
node tools/publish-release.mjs --allow-dirty                # draft
node tools/publish-release.mjs --allow-dirty --publish      # live, only once the user has said so
```

It finds the newest archive in `dist/` on its own, reads the commit out of the filename, picks the
tag, writes the notes from the package's own `FIRMWARE.md`, and uploads. Prints the release URL.

| flag | when |
|---|---|
| `--publish` | go live instead of drafting. **Confirm with the user first.** |
| `--allow-dirty` | same tree reality as above. The notes gain a visible warning that the images cannot be rebuilt from the tag. |
| `--tag <tag>` | override the automatic tag. |
| `--zip <file>` | publish a specific archive rather than the newest. |
| `--prerelease` | mark it as such on the page. |
| `--note "..."` | a "What changed" section at the top of the notes. Markdown, multi-line. Worth writing for anything but a rebuild — a release page with no changelog is one somebody has to `git log` to understand. |
| `--notes-only` | render the notes to `target/release-notes.md` and print them, upload nothing. **Use this to show the user what the release will say before publishing.** |

### Tags follow this repository, not semver

Existing releases are dates — `2023-08-26`, `2023-08-26B`, `2023-12-20`, `2024-10-02`. The script
takes today's date and adds `B`, `C`, … if that tag is taken, because re-releasing on the same day
is ordinary here. Do not invent `v1.0.0`.

### If the firmware sources are dirty, that is the thing to fix first

`--allow-dirty` exists, but reach for it last. The archive is named for the commit, so a dirty
build published twice in a day produces two releases whose assets have the *same filename* and
different firmware — which the sha256 table in the notes documents and the download filename does
not. Committing the firmware work first gives a distinct name and images that can be rebuilt from
the tag. Committing is the user's call: ask, do not assume.

### What it refuses

- **A dirty package**, unless `--allow-dirty`. Handing the zip to a colleague directly needs no
  such flag; publishing it *against a commit* is what makes the provenance a claim.
- **A commit that is not on the remote**, and separately **a commit this clone does not have** —
  different messages, because they lead to different actions.
- **A tag that already exists**, when one was passed explicitly.

### The notes repeat the quarantine instructions on purpose

`README.txt` inside the zip covers it, and that is exactly one step too late: a file downloaded
from a GitHub release is quarantined by definition, so the first thing that happens to the
recipient is the failure the README explains — and they cannot read the README without unzipping
the thing macOS has just refused. It has to be on the page they are already looking at.

---

## What neither script does, and what to say about it

- **macOS only.** `package.mjs` refuses on Windows by design. The Windows layout needs the CEF
  payload to travel (`libcef.dll` is an import library there, resolved before `main` runs whether
  anything calls it or not) and that is not something this could produce or check from a Mac.
- **Notarisation.** Ad hoc by default, so the recipient must clear quarantine either way.
- **Real hardware.** The verify pass runs `--simulate`. Flashing an actual board with the packaged
  copy is the one check nothing here replaces — mention it if the package is going somewhere it
  matters.

## Sanity-checking by hand, if asked

```sh
unzip -l dist/PortalTestBench-*.zip | head -20      # top-level folder, .app, README.txt, FIRMWARE.md
cat PortalTestBench/target/package/*/README.txt     # what the recipient reads first
cat PortalTestBench/target/package/*/FIRMWARE.md    # load address, size, banner and sha256 per image
```

To rehearse the recipient's experience end to end — worth doing when the README or the notes have
changed, and not otherwise:

```sh
ditto -x -k dist/PortalTestBench-*.zip /tmp/recipient
xattr -w com.apple.quarantine "0083;0;Safari;" /tmp/recipient/PortalTestBench-*/PortalTestBench.app
xattr -dr com.apple.quarantine /tmp/recipient/PortalTestBench-*/PortalTestBench.app   # the README's step 1
open /tmp/recipient/PortalTestBench-*/PortalTestBench.app --args --simulate
```

## Where the pieces live

- `PortalTestBench/tools/package.mjs` — the build. Its header comment carries the reasoning.
- `PortalTestBench/tools/publish-release.mjs` — the release. Likewise.
- `PortalTestBench/tools/bundle-macos.mjs` — the `.app` and the signing, called with `--resources`.
- `README.md` at the repository root, "Packaging" — the layout and the resource-resolution table.
- `portal_swd::artefacts::{resource_roots, artefact_root}` — how a packaged copy finds its firmware.
