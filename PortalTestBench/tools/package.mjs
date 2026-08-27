// Wrap PortalTestBench, its CLI, its plans and a full firmware set into one archive that needs
// nothing on the far machine.
//
//     node tools/package.mjs [--skip-build] [--allow-dirty] [--profile release|debug]
//                            [--sign <identity>] [--skip-verify]
//
// ## What "needs nothing" has to mean
//
// The recipient is going to flash boards with this. So the bar is not "it starts": it is that a
// person who has never held this repository can unzip it, plug in an ST-Link, and program a
// module. Three things follow, and each is checked rather than assumed:
//
//  1. **No external libraries.** `otool -L` on the staged binaries must report only system
//     frameworks and `/usr/lib`. probe-rs reaches USB through `nusb`, which is pure Rust, so
//     there is no libusb to install and no Homebrew in the story -- but that is a property of a
//     dependency graph that can change under a `cargo update`, which is why `checkLinkage` below
//     asserts it at package time instead of trusting this paragraph.
//  2. **The whole firmware set.** All four `APPLICATION_ENVS` -- optical and mechanical, each at
//     the v6 base and at the legacy base -- plus the built bootloader and the committed reference
//     image. A package carrying three of the four is a package that works until somebody puts the
//     other PCB revision in the fixture.
//  3. **The ELF beside every image.** Not decoration: `Discovery::run_check_for` resolves
//     `g_liveness_counter` out of it, and without one the run-check degrades to "the vector table
//     is where it should be" -- which passes on a board stuck in `HardFault_Handler`.
//
// ## Why the payload mirrors a repository
//
// `firmware/` is shaped like a built checkout -- `PortalFW/.pio/build/<env>/firmware.bin` -- so
// `portal_swd::artefacts::discover_in` serves a package and a developer's tree with one
// implementation. There is no second discovery path to keep in agreement, which is the whole
// reason the payload looks redundant rather than tidy.
//
// ## macOS only, and it says so
//
// The Windows layout is documented in the repository README and is not built here. It is not a
// copy of this with different slashes: `libcef.dll` is an *import library* there, so the CEF
// payload has to travel even though nothing calls it, and that is a claim this script could
// neither produce nor check from a Mac. Refusing is better than shipping an untested branch.

import crypto from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

import { done, fail, IS_MACOS, main, run, step, tryRun, warn } from '../../tools/lib/proc.mjs';

const app = path.resolve(import.meta.dirname, '..');
const repo = path.resolve(app, '..');

const APP_NAME = 'PortalTestBench';
const DEFAULT_MACOS_SIGN_IDENTITY = 'Developer ID Application: elliot Woods (CGB4H2337N)';

/**
 * Every environment the package ships, and where it goes in the payload.
 *
 * The list is `APPLICATION_ENVS` in `portal-swd/src/artefacts.rs` plus the bootloader. It is
 * duplicated here rather than read from that file, and `verify` is what stops the two drifting:
 * it runs the packaged bench and refuses a package whose `missing` list is not empty, which is
 * exactly "discovery expected an image this script did not ship".
 *
 * `application_bank_optical_bringup`, `no_bootloader` and `debug_no_bootloader` are deliberately
 * absent. Discovery does not offer them, and the last two link at 0x08000000 and would program,
 * verify and never run.
 */
const ENVS = [
  { env: 'application_bank_optical', project: 'PortalFW', label: 'optical, PCB v6' },
  { env: 'application_bank_mechanical', project: 'PortalFW', label: 'mechanical, PCB v4' },
  { env: 'application_bank_optical_legacy_base', project: 'PortalFW', label: 'optical, PCB v6, legacy base' },
  { env: 'application_bank_mechanical_legacy_base', project: 'PortalFW', label: 'mechanical, PCB v4, legacy base' },
  { env: 'bootloader', project: 'PortalBootloader', label: 'bootloader' },
];

/** How many artefacts the packaged bench must list. The five above plus the reference image. */
const EXPECTED_ARTEFACTS = ENVS.length + 1;

const APP_DESCRIPTOR_OFFSET = 0xc0;
const APP_DESCRIPTOR_MAGIC = 'KC79APP1';
const FLASH_BASE = 0x0800_0000;

function parseArgs(argv) {
  const options = {
    skipBuild: false,
    allowDirty: false,
    profile: 'release',
    sign: DEFAULT_MACOS_SIGN_IDENTITY,
    verify: true,
  };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    const next = () => {
      const value = argv[i + 1];
      if (!value) fail(`${arg} needs a value`);
      i += 1;
      return value;
    };
    if (arg === '--skip-build') options.skipBuild = true;
    else if (arg === '--allow-dirty') options.allowDirty = true;
    else if (arg === '--skip-verify') options.verify = false;
    else if (arg === '--profile') options.profile = next();
    else if (arg === '--sign') options.sign = next();
    else fail(`unknown argument \`${arg}\``);
  }
  if (!['debug', 'release'].includes(options.profile)) {
    fail(`--profile must be debug or release, not \`${options.profile}\``);
  }
  return options;
}

function sha256(file) {
  return crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
}

function copyTree(src, dst) {
  fs.mkdirSync(dst, { recursive: true });
  for (const entry of fs.readdirSync(src, { withFileTypes: true })) {
    const from = path.join(src, entry.name);
    const to = path.join(dst, entry.name);
    if (entry.isDirectory()) copyTree(from, to);
    else fs.copyFileSync(from, to);
  }
}

/**
 * Where an image says it was linked, read the way `portal_swd::image::image_base` reads it.
 *
 * The descriptor at `base + 0xC0` wins. Without one the only honest answer is "not stated" -- this
 * deliberately does not fall back to inferring the legacy base from the reset vector, because a
 * manifest is a record and an inference in a record reads as a measurement.
 */
function loadAddress(file, project) {
  if (project === 'PortalBootloader') return FLASH_BASE;
  const bytes = fs.readFileSync(file);
  const magic = bytes.subarray(APP_DESCRIPTOR_OFFSET, APP_DESCRIPTOR_OFFSET + 8).toString('latin1');
  if (magic !== APP_DESCRIPTOR_MAGIC) return null;
  return bytes.readUInt32LE(APP_DESCRIPTOR_OFFSET + 8);
}

/** The banner an image carries, which is the only place a `.bin` names its own build. */
function banner(file) {
  const text = fs.readFileSync(file).toString('latin1');
  const match = text.match(/(Portal v|Bootloader v)[\x20-\x7e]{0,63}/);
  return match ? match[0].trim() : null;
}

function platformPin(project) {
  const ini = fs.readFileSync(path.join(repo, project, 'platformio.ini'), 'utf8');
  return ini.match(/^platform\s*=\s*(ststm32@\S+)/m)?.[1] ?? 'unknown';
}

/**
 * The short commit, plus `-dirty` when the firmware sources have uncommitted changes.
 *
 * `git describe` was the obvious choice and gave `2023-12-20-108-g8c6834d8-dirty` -- the distance
 * from a three-year-old tag, which says nothing anyone here would use and makes the folder name
 * unreadable. The README names the archive `<sha>`, and a sha is what it means.
 */
function gitDescribe(dirty) {
  const sha = tryRun('git', ['-C', repo, 'rev-parse', '--short', 'HEAD']).stdout || 'unknown';
  return dirty ? `${sha}-dirty` : sha;
}

/**
 * Refuse a tree whose firmware sources have uncommitted changes.
 *
 * `PortalFW/set_build_date.py` compiles the git description into `Version.h` and therefore into
 * the image, so a package whose manifest names one commit and whose firmware reports another is
 * worse than no manifest. Scoped to the directories that actually reach the images -- an edited
 * README in `RouterRS` has nothing to do with what a board will run.
 */
function checkClean(allowDirty) {
  const paths = ['PortalFW', 'PortalBootloader'];
  const dirty = tryRun('git', ['-C', repo, 'status', '--porcelain', '--', ...paths]).stdout;
  if (!dirty) return false;
  if (!allowDirty) {
    fail(
      `the firmware sources have uncommitted changes:\n${dirty}\n` +
        '  The git description is compiled into the image, so the manifest would name a commit\n' +
        '  the firmware does not report. Commit them, or pass --allow-dirty and accept that the\n' +
        '  package records a dirty build.',
    );
  }
  warn('--allow-dirty: the firmware sources are modified. The manifest records this.');
  return true;
}

/**
 * Every dynamic library the staged binaries need, refusing anything the far machine might not have.
 *
 * This is the check that keeps "needs nothing installed" true rather than remembered. A
 * `cargo update` that swapped `nusb` for `rusb` would introduce a `/opt/homebrew/lib/libusb-1.0`
 * link, the package would keep building, and it would fail on the first machine without Homebrew
 * -- with a dyld error naming a library the recipient has never heard of.
 */
function checkLinkage(binaries) {
  const allowed = [/^\/System\/Library\//, /^\/usr\/lib\//];
  const foreign = [];
  for (const binary of binaries) {
    const listed = tryRun('otool', ['-L', binary]);
    if (!listed.ok) {
      warn(`otool could not read ${path.basename(binary)}; linkage is unchecked`);
      continue;
    }
    for (const line of listed.stdout.split('\n').slice(1)) {
      const lib = line.trim().split(' ')[0];
      if (!lib || !lib.includes('/')) continue;
      if (!allowed.some((pattern) => pattern.test(lib))) foreign.push(`${path.basename(binary)}: ${lib}`);
    }
  }
  if (foreign.length > 0) {
    fail(
      `these binaries link libraries that are not part of macOS:\n  ${foreign.join('\n  ')}\n` +
        '  The package would fail on any machine without them. This is the check that keeps\n' +
        '  "no Homebrew required" true; see the note at the top of this file.',
    );
  }
  step(`Linkage: system frameworks and /usr/lib only (${binaries.length} binaries)`);
}

function stageFirmware(payload, describe, dirty) {
  const rows = [];
  for (const { env, project, label } of ENVS) {
    const from = path.join(repo, project, '.pio/build', env);
    const bin = path.join(from, 'firmware.bin');
    if (!fs.existsSync(bin)) {
      fail(`${project}/.pio/build/${env}/firmware.bin is missing.\n  Run: node tools/package.mjs (without --skip-build)`);
    }
    const to = path.join(payload, 'firmware', project, '.pio/build', env);
    fs.mkdirSync(to, { recursive: true });
    fs.copyFileSync(bin, path.join(to, 'firmware.bin'));

    // The ELF travels with the image. `Discovery::run_check_for` resolves `g_liveness_counter`
    // from it, and a run-check without one cannot tell a running board from a hard-faulted one.
    const elf = path.join(from, 'firmware.elf');
    const hasElf = fs.existsSync(elf);
    if (hasElf) fs.copyFileSync(elf, path.join(to, 'firmware.elf'));
    else warn(`${env} has no firmware.elf; its run-check will have no liveness symbol`);

    const base = loadAddress(bin, project);
    rows.push({
      env,
      label,
      base: base === null ? 'not stated' : `0x${base.toString(16).toUpperCase().padStart(8, '0')}`,
      bytes: fs.statSync(bin).size,
      banner: banner(bin) ?? '—',
      sha256: sha256(bin),
      elf: hasElf,
      pin: platformPin(project),
    });
  }

  // The committed reference bootloader, which is what a board still on v4/v5 was fielded with.
  const referenceFrom = path.join(repo, 'PortalBootloader/reference');
  const referenceTo = path.join(payload, 'firmware/PortalBootloader/reference');
  fs.mkdirSync(referenceTo, { recursive: true });
  for (const name of fs.readdirSync(referenceFrom)) {
    if (!/\.(bin|elf)$/.test(name)) continue;
    const file = path.join(referenceFrom, name);
    fs.copyFileSync(file, path.join(referenceTo, name));
    if (name.endsWith('.bin')) {
      rows.push({
        env: `reference/${name}`,
        label: 'fielded v4/v5 bootloader',
        base: `0x${FLASH_BASE.toString(16).toUpperCase().padStart(8, '0')}`,
        bytes: fs.statSync(file).size,
        banner: banner(file) ?? '—',
        sha256: sha256(file),
        elf: fs.existsSync(file.replace(/\.bin$/, '.elf')),
        pin: 'committed binary',
      });
    }
  }

  const manifest = [
    '# Firmware in this package',
    '',
    `Built from \`${describe}\`${dirty ? ' with uncommitted changes in the firmware sources' : ''}.`,
    '',
    'Nothing reads this file. It is the only place a `.bin` can say what it is, and the load',
    'address is the field that matters: the two application banks overlap, so an image flashed to',
    'the wrong one programs cleanly, verifies cleanly and hard-faults on the first absolute',
    'reference. The bench reads each address out of the image itself rather than from here.',
    '',
    '| image | what | load address | bytes | banner | ELF | sha256 |',
    '|---|---|---|---|---|---|---|',
    ...rows.map(
      (r) =>
        `| \`${r.env}\` | ${r.label} | \`${r.base}\` | ${r.bytes} | ${r.banner} | ${r.elf ? 'yes' : 'no'} | \`${r.sha256}\` |`,
    ),
    '',
    `PlatformIO: ${[...new Set(rows.map((r) => r.pin))].join(', ')}`,
    '',
  ].join('\n');
  fs.writeFileSync(path.join(payload, 'firmware/MANIFEST.md'), manifest);
  return rows;
}

function readme({ describe, signed, dirty, rows }) {
  const images = rows
    .filter((r) => !r.env.startsWith('reference/'))
    .map((r) => `    ${r.env.padEnd(40)} ${r.label}`)
    .join('\n');
  // Kept short so the sentence below wraps inside 96 columns whichever branch it takes. The
  // first draft interpolated a clause and produced a 110-character line in a plain-text file.
  const signature = signed ? 'with a Developer ID but not notarised' : 'ad hoc rather than notarised';
  return `PortalTestBench
===============

Flash, provision and test a single KC79 Portal module. Everything it is going to need is in this
folder: the application, the CLI, the test plans, and a full set of firmware for both PCB
revisions. There is nothing to install first -- no Homebrew, no Rust, no Node, no PlatformIO, no
libusb. Plug in an ST-Link and go.

Built from ${describe}.${dirty ? '\n(The "-dirty" is honest: the firmware sources had uncommitted changes when this was built.\nFIRMWARE.md records the sha256 of every image, which is the part that identifies them.)' : ''}


1. Let macOS run it
-------------------

Anything copied from another machine is quarantined by macOS, and this app is signed
${signature} -- so it will not open while that flag is
set. You will see "PortalTestBench is damaged and can't be opened", or "cannot be opened because
Apple cannot check it for malicious software". Neither message means what it says. It means the
quarantine flag is still on.

Clear it. Open Terminal, type the following INCLUDING the trailing space, then drag
PortalTestBench.app from the Finder onto the Terminal window so the path fills itself in, and
press return:

    xattr -dr com.apple.quarantine 

There will be no output. That is what success looks like. Double-click the app and it opens.

If you would rather not touch Terminal: double-click the app and let it be refused, then go to
System Settings -> Privacy & Security, scroll to the bottom, and press "Open Anyway" next to the
message about PortalTestBench. You have to let it fail first -- the button only appears afterwards.
On macOS 15 and later this is the only route through the interface; the old right-click -> Open
trick no longer works for an app that has not been notarised.

Either way it is one-time. macOS remembers.

You can keep the app in this folder or drag it to /Applications. It does not mind which, and it
finds its firmware either way -- everything it needs is inside the .app itself.


2. Run it
---------

Double-click PortalTestBench.app. A window opens, and the same interface is served at

    http://127.0.0.1:8770

so you can also drive it from a browser, here or from another machine on the same network.

To have a look with no hardware attached, run it with --simulate. In Terminal, type this
(again with the trailing space), drag the app on, and add the rest:

    open  --args --simulate


3. Flash a board
----------------

  1. Plug in the ST-Link and wire it to the fixture.
  2. Press "Rescan all", then pick your probe under "1 ST-Link probe".
  3. Under "2 Firmware banks", pick a bootloader and an application. In this package:

${images}
    reference/BootloaderRS485-2023-08-26.bin   the bootloader fielded on v4/v5 boards

     Which application: optical is PCB v6, mechanical is PCB v4. The "legacy base" pair is for a
     board still carrying a v4 or v5 bootloader and NOT having it replaced in this pass. If you
     are writing the bootloader too -- the usual case -- you want the plain pair.

     Which bootloader: the built one. The reference image is the old v4 that fielded boards
     shipped with, kept so you can put a board back the way you found it. It cannot be used for
     provisioning, and the bench will say so rather than let you.

  4. Set the serial number, then press "Flash / Provision now" twice within five seconds.

Each bank also offers "Keep existing" and "Erase", so you can write one bank and leave the other
alone -- or deliberately wipe it.

To flash something that is not in this list -- a build somebody has just sent you -- drag the .bin
or .elf onto the window. The bench identifies it, works out which bank it belongs in, refuses it if
it is not firmware for this part, and selects it ready to flash. An .elf is converted to its flash
image on the way in.

FIRMWARE.md, beside this file, records the load address, size and sha256 of every image here.


4. Where it puts things
-----------------------

Session logs and the provisioning database are written to

    ~/Library/Application Support/AuroraVision/av-frameworks/portal-test-bench

and not into this folder, so the package stays read-only and your evidence survives moving it.


5. The CLI
----------

There is a second executable inside the bundle, at

    PortalTestBench.app/Contents/MacOS/ptb

It talks to a running bench over the same HTTP API the window uses, so a person at the GUI and a
script can drive one bench at the same time. \`ptb --help\` lists what it does.


Signature: ${signed ? 'Developer ID, not notarised' : 'ad hoc'}.
`;
}

/**
 * Unpack the archive somewhere clean and make the bench prove it can see its own firmware.
 *
 * This is the difference between shipping a zip and shipping a working one, and it is cheap: the
 * failure it catches -- a payload that is present but in the wrong place, so `artefact_root` falls
 * through to a repository path that does not exist on the far machine -- produces an application
 * that starts perfectly and offers nothing to flash. Nothing else in the build would notice.
 *
 * It runs from a temporary directory, not the source tree, and with the environment overrides
 * cleared, so it cannot accidentally pass by finding the developer's own firmware.
 */
function verifyPackage(zip, folder, port) {
  // `realpathSync`, because `os.tmpdir()` is `/var/folders/...` and `/var` is a symlink to
  // `/private/var`. Without it the "is the firmware inside the package" check compares a resolved
  // path against an unresolved one and fails on a package that is perfectly correct -- which is
  // what it did on the first run.
  const scratch = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'ptb-package-')));
  try {
    run('ditto', ['-x', '-k', zip, scratch]);
    const root = path.join(scratch, folder);
    const binary = path.join(root, `${APP_NAME}.app/Contents/MacOS/portal-test-bench`);
    if (!fs.existsSync(binary)) {
      fail(`the archive does not contain ${folder}/${APP_NAME}.app/Contents/MacOS/portal-test-bench`);
    }
    for (const name of ['README.txt', 'FIRMWARE.md']) {
      if (!fs.existsSync(path.join(root, name))) fail(`the archive does not contain ${folder}/${name}`);
    }

    const env = { ...process.env };
    for (const key of ['PORTAL_FIRMWARE_DIR', 'PORTAL_FIRMWARE_ROOT', 'PORTAL_TEST_BENCH_PLANS']) delete env[key];
    const child = tryRun('/bin/sh', [
      '-c',
      `"${binary}" --headless --simulate --port ${port} >/dev/null 2>&1 & echo $!`,
    ], { env, cwd: scratch });
    const pid = Number(child.stdout.trim());
    try {
      let body = null;
      for (let attempt = 0; attempt < 40; attempt += 1) {
        const probe = tryRun('curl', ['-s', '--max-time', '1', `http://127.0.0.1:${port}/api/bench/firmware`]);
        if (probe.ok && probe.stdout.startsWith('{')) { body = JSON.parse(probe.stdout); break; }
        tryRun('/bin/sh', ['-c', 'sleep 0.5']);
      }
      if (!body) fail('the unpacked bench did not answer /api/bench/firmware');
      if ((body.missing ?? []).length > 0) {
        fail(`the unpacked bench is missing firmware it expected:\n  ${body.missing.map((m) => `${m.label}: ${m.hint}`).join('\n  ')}`);
      }
      const found = body.found ?? [];
      if (found.length !== EXPECTED_ARTEFACTS) {
        fail(`the unpacked bench lists ${found.length} artefacts, expected ${EXPECTED_ARTEFACTS}:\n  ${found.map((a) => a.id).join('\n  ')}`);
      }
      const tooLarge = found.filter((a) => !a.fits);
      if (tooLarge.length > 0) fail(`these images do not fit their bank: ${tooLarge.map((a) => a.id).join(', ')}`);
      if (!path.resolve(body.root ?? '').startsWith(path.resolve(scratch))) {
        fail(`the unpacked bench resolved its firmware to ${body.root}, which is outside the package`);
      }
      step(`Verified: ${found.length} artefacts, none missing, all fit, root inside the package`);
    } finally {
      if (Number.isFinite(pid)) tryRun('kill', [String(pid)]);
    }
  } finally {
    fs.rmSync(scratch, { recursive: true, force: true });
  }
}

main(() => {
  const options = parseArgs(process.argv.slice(2));
  if (!IS_MACOS) {
    fail(
      'this script builds the macOS package only.\n' +
        '  The Windows layout is documented in the repository README and is not a copy of this one:\n' +
        '  libcef.dll is an import library there, so the CEF payload has to travel even though\n' +
        '  nothing calls it. That is not something this script could produce or check from a Mac.',
    );
  }

  const dirty = checkClean(options.allowDirty);
  const describe = gitDescribe(dirty);

  if (!options.skipBuild) {
    step('Building the application');
    run('node', ['tools/build.mjs', ...(options.profile === 'release' ? ['--release'] : [])], { cwd: app });
    step('Building firmware');
    for (const { env, project } of ENVS) {
      run(path.join(os.homedir(), '.platformio/penv/bin/pio'), ['run', '-e', env], {
        cwd: path.join(repo, project),
      });
    }
  }

  const work = path.join(app, 'target', 'package');
  fs.rmSync(work, { recursive: true, force: true });
  const payload = path.join(work, 'payload');
  // The staging directory's *name* is the folder the recipient ends up with, because `ditto
  // --keepParent` archives the directory rather than its contents. It was `stage`, and the first
  // verify run is what said so -- an archive that unpacks into a folder called "stage" is a small
  // thing that reads as a mistake in a package somebody is about to trust with a board.
  const folder = `${APP_NAME}-${describe}`;
  const stage = path.join(work, folder);
  fs.mkdirSync(payload, { recursive: true });
  fs.mkdirSync(stage, { recursive: true });

  step('Staging plans');
  copyTree(path.join(app, 'plans'), path.join(payload, 'plans'));

  step('Staging firmware');
  const rows = stageFirmware(payload, describe, dirty);
  for (const row of rows) console.log(`    ${row.env.padEnd(42)} ${String(row.bytes).padStart(7)} B  ${row.base}`);

  step('Bundling');
  run('node', [
    'tools/bundle-macos.mjs',
    '--profile', options.profile,
    '--resources', payload,
    '--out', stage,
    '--sign', options.sign,
  ], { cwd: app });

  const bundle = path.join(stage, `${APP_NAME}.app`);
  checkLinkage([
    path.join(bundle, 'Contents/MacOS/portal-test-bench'),
    path.join(bundle, 'Contents/MacOS/ptb'),
  ].filter((file) => fs.existsSync(file)));

  fs.writeFileSync(path.join(stage, 'README.txt'), readme({ describe, signed: options.sign !== '-', dirty, rows }));
  fs.copyFileSync(path.join(payload, 'firmware/MANIFEST.md'), path.join(stage, 'FIRMWARE.md'));

  const dist = path.join(repo, 'dist');
  fs.mkdirSync(dist, { recursive: true });
  const zip = path.join(dist, `${APP_NAME}-${describe}-macos-${process.arch}.zip`);
  fs.rmSync(zip, { force: true });

  // `ditto`, not `zip`. The archive carries a signed bundle, and `zip` does not preserve the
  // extended attributes the signature lives in -- an archive made with it unpacks into an app
  // macOS refuses for a reason that looks like corruption. `--sequesterRsrc` is what puts them
  // somewhere a round trip can find them again.
  step('Archiving');
  run('ditto', ['-c', '-k', '--sequesterRsrc', '--keepParent', stage, zip]);

  if (options.verify) verifyPackage(zip, folder, 8779);

  const size = tryRun('du', ['-h', zip]).stdout.split(/\s+/)[0];
  console.log('');
  done(`${path.basename(zip)}  ${size}`);
  console.log(`  ${zip}`);
  console.log('');
  console.log('  The recipient unzips it and reads README.txt. The first step there is clearing');
  console.log('  the macOS quarantine flag, which they will need whatever they do with it.');
});
