// Wrap PortalTestBench and the firmware it flashes into one thing that can be handed to someone.
//
//     node tools/package.mjs                              # for this platform, release
//     node tools/package.mjs --profile debug              # what you have already built
//     node tools/package.mjs --sign "Developer ID Application: ..."
//     node tools/package.mjs --allow-dirty                # package an uncommitted tree, and say so
//     node tools/package.mjs --skip-build                 # use what is in target/ and .pio/
//
// ## What a package has to solve
//
// A bench binary built here resolves its plans, its report destination and every firmware artefact
// against `CARGO_MANIFEST_DIR` -- paths baked in at compile time, which are exactly right for a
// developer and meaningless on a machine that has never held this repository. Three changes made
// that survivable and this script is the fourth: it puts the files where the runtime now looks.
//
//   * `plans/` and `firmware/` beside the executable -- `Contents/Resources` inside a macOS
//     bundle -- read by `plans_dir()` and `portal_swd::artefacts::artefact_root()`, each of which
//     prefers what was shipped over what was compiled in.
//   * The report destination is *not* here. A packaged run writes to the platform's per-user state
//     directory, because a `.app` in `/Applications` cannot write beside itself.
//
// ## Why `resources/firmware` looks like a build tree
//
// Because it is one. `firmware/PortalFW/.pio/build/<env>/firmware.bin` is the same shape a
// developer's tree has, so `portal_swd::artefacts::discover_in` serves both with one
// implementation and there is no second discovery path to keep in agreement. `MANIFEST.md` beside
// it is for a human -- nothing reads it -- and records what a `.bin` cannot say about itself:
// which commit, which PlatformIO release, which environment, and the sha256.

import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';

import {
  IS_MACOS,
  IS_WINDOWS,
  commas,
  done,
  exeName,
  fail,
  main,
  repoRoot,
  run,
  step,
  tryRun,
  warn,
} from './lib/proc.mjs';
import { ENVIRONMENTS, artefactPaths, verifyBuilt } from './firmware.mjs';

const root = repoRoot();
const bench = path.join(root, 'PortalTestBench');

/** Shipped beside the bench. `av-gui-subprocess` is added on the platforms that launch it. */
const BINARIES = ['portal-test-bench', 'ptb', 'av-gui-subprocess'];

function parseArgs(argv) {
  const options = {
    profile: 'release',
    sign: '-',
    out: path.join(root, 'dist'),
    allowDirty: false,
    skipBuild: false,
  };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    const next = () => {
      const value = argv[i + 1];
      if (!value) fail(`${arg} needs a value`);
      i += 1;
      return value;
    };
    if (arg === '--profile') options.profile = next();
    else if (arg === '--sign') options.sign = next();
    else if (arg === '--out') options.out = path.resolve(next());
    else if (arg === '--allow-dirty') options.allowDirty = true;
    else if (arg === '--skip-build') options.skipBuild = true;
    else fail(`unknown argument \`${arg}\``);
  }
  if (!['debug', 'release'].includes(options.profile)) {
    fail(`--profile must be debug or release, not \`${options.profile}\``);
  }
  return options;
}

function sha256(file) {
  return createHash('sha256').update(fs.readFileSync(file)).digest('hex');
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
 * What this tree was, at the moment it was packaged.
 *
 * A dirty tree is refused rather than warned about, because `PortalFW/set_build_date.py` compiles
 * the same git description into `Version.h` and therefore into the firmware itself. A package
 * whose MANIFEST says one commit and whose firmware reports another is worse than no manifest.
 */
function provenance(allowDirty) {
  const head = tryRun('git', ['-C', root, 'rev-parse', 'HEAD']);
  const short = tryRun('git', ['-C', root, 'rev-parse', '--short', 'HEAD']);
  const dirty = tryRun('git', ['-C', root, 'status', '--porcelain']);
  if (!head.ok) {
    warn('Not a git checkout, or git is unavailable: the manifest will not name a commit.');
    return { sha: 'unknown', short: 'unknown', dirty: false };
  }
  const isDirty = dirty.ok && dirty.stdout.length > 0;
  if (isDirty && !allowDirty) {
    fail(
      'The working tree has uncommitted changes.\n' +
        '\n  A package records the commit it was built from, and PortalFW compiles the same\n' +
        '  description into Version.h -- so a dirty tree produces firmware that reports a commit\n' +
        '  whose contents are not what was built.\n' +
        '\n  Commit, or pass --allow-dirty to package anyway (the manifest will say so).\n' +
        `\n${dirty.stdout.split('\n').slice(0, 12).join('\n')}`,
    );
  }
  return { sha: head.stdout, short: short.stdout, dirty: isDirty };
}

/** The PlatformIO release each project pins, read from the ini rather than from memory. */
function pinnedPlatform(project) {
  const ini = fs.readFileSync(path.join(root, project, 'platformio.ini'), 'utf8');
  const match = ini.match(/^\s*platform\s*=\s*(\S+)/m);
  return match ? match[1] : 'unpinned';
}

function stageFirmware(resources, artefacts, meta) {
  const firmware = path.join(resources, 'firmware');
  const rows = [];

  for (const artefact of artefacts) {
    const dst = path.join(firmware, artefact.dir);
    fs.mkdirSync(dst, { recursive: true });
    fs.copyFileSync(artefact.bin, path.join(dst, 'firmware.bin'));
    if (artefact.elf) fs.copyFileSync(artefact.elf, path.join(dst, 'firmware.elf'));
    rows.push({
      env: artefact.env,
      label: artefact.label,
      base: artefact.base,
      bytes: artefact.bytes,
      free: artefact.limit - artefact.bytes,
      elf: Boolean(artefact.elf),
      sha: sha256(artefact.bin),
      platform: pinnedPlatform(artefact.project),
    });
  }

  // The committed reference bootloader travels too. `artefacts.rs` offers it whenever it is
  // present and prefers a built one over it, so shipping both costs 22 kB and gives an operator a
  // fielded image to fall back to without another download.
  const referenceSrc = path.join(root, 'PortalBootloader', 'reference');
  if (fs.existsSync(referenceSrc)) {
    copyTree(referenceSrc, path.join(firmware, 'PortalBootloader', 'reference'));
  }

  const manifest = [
    '# Firmware in this package',
    '',
    `Built from \`${meta.short}\`${meta.dirty ? ' **with uncommitted changes**' : ''} on ${meta.date}.`,
    '',
    'Every image below was checked before it was copied here: its size against its bank, its',
    'reset vector against that bank, and its initial stack pointer against this part\'s 36 kB of',
    'SRAM. The bench checks them again when it loads one.',
    '',
    '| Environment | Loads at | Bytes | Free | ELF | PlatformIO | sha256 |',
    '|---|---|---|---|---|---|---|',
    ...rows.map(
      (r) =>
        `| \`${r.env}\` | \`0x${r.base.toString(16).padStart(8, '0')}\` | ${commas(r.bytes)} | ` +
        `${commas(r.free)} | ${r.elf ? 'yes' : 'no'} | \`${r.platform}\` | \`${r.sha.slice(0, 16)}…\` |`,
    ),
    '',
    '`application_bank_optical` is PCB **v6** (optical home switch) and is the production default.',
    '`application_bank_mechanical` is PCB **v4** (`-D HOME_SWITCH_LEGACY`). Nothing on the board',
    'says which revision it is, so the bench offers both and an operator picks.',
    '',
    'The ELF beside an image is not decoration: it is where the run check resolves',
    '`g_liveness_counter`. An image shipped without one still flashes and still verifies, and the',
    'bench will say it cannot confirm the firmware is running.',
    '',
    `Reference bootloader: \`PortalBootloader/reference/\`, the image fielded in 2023. The bench`,
    'prefers a built bootloader over it and offers both.',
    '',
  ].join('\n');
  fs.writeFileSync(path.join(firmware, 'MANIFEST.md'), `${manifest}\n`);

  return rows;
}

function readme(meta, rows, platform) {
  const exe = platform === 'windows' ? 'portal-test-bench.exe' : 'PortalTestBench.app';
  return `Portal Test Bench
=================

Built from ${meta.short}${meta.dirty ? ' (with uncommitted changes)' : ''} on ${meta.date}.

A bench instrument for a single portal module: flash it, connect to it, drive it, watch it, and
come away with evidence. It carries the firmware it flashes -- nothing here needs the KC79.Reworld
repository, Rust, Node or PlatformIO.

Running it
----------
${
  platform === 'windows'
    ? `    portal-test-bench.exe             a window, and http://127.0.0.1:8770
    portal-test-bench.exe --headless  the same page, no window
    portal-test-bench.exe --simulate  a modelled module: no probe, no port, no board
    ptb.exe state                     the same bench, for an agent or a script

Needs the WebView2 Runtime, which Windows 11 ships.`
    : `    open PortalTestBench.app
    PortalTestBench.app/Contents/MacOS/portal-test-bench --headless
    PortalTestBench.app/Contents/MacOS/ptb state

This bundle is signed ad hoc, not notarised, so the first launch needs
Finder -> right-click -> Open (or: xattr -dr com.apple.quarantine PortalTestBench.app).`
}

Flashing
--------
Connect an ST-Link. The firmware picker offers:

${rows.map((r) => `    ${r.env.padEnd(30)} loads at 0x${r.base.toString(16).padStart(8, '0')}`).join('\n')}

application_bank_optical is PCB v6 and is the default. application_bank_mechanical is PCB v4.
Nothing on the board says which it is, so you pick.

resources/firmware/MANIFEST.md records what each image is, its size, and its sha256.

Where your sessions go
----------------------
Session evidence is written to the per-user state directory, not into this package:

${
  platform === 'windows'
    ? '    %LOCALAPPDATA%\\AuroraVision\\av-frameworks\\portal-test-bench'
    : '    ~/Library/Application Support/AuroraVision/av-frameworks/portal-test-bench'
}

Set PORTAL_TEST_BENCH_REPORTS to put them somewhere else. PORTAL_FIRMWARE_DIR points the picker
at a different firmware tree, and PORTAL_TEST_BENCH_PLANS at a different set of plans.
`;
}

main(() => {
  const options = parseArgs(process.argv.slice(2));
  const meta = { ...provenance(options.allowDirty), date: new Date().toISOString().slice(0, 10) };

  // --- build ---------------------------------------------------------------------------
  if (!options.skipBuild) {
    step('Building firmware');
    run('node', [path.join(root, 'tools', 'build-firmware.mjs')]);
    step('Building the bench');
    run('node', [
      path.join(bench, 'tools', 'build.mjs'),
      ...(options.profile === 'release' ? ['--release'] : []),
    ]);
  }

  // --- check what we are about to ship -------------------------------------------------
  const artefacts = ENVIRONMENTS.map((environment) => {
    const { bin } = artefactPaths(environment, root);
    if (!fs.existsSync(bin)) {
      fail(
        `${environment.env} is not built.\n` +
          '  Run: node tools/build-firmware.mjs   (or drop --skip-build)',
      );
    }
    return verifyBuilt(environment, root);
  });

  const targetDir = path.join(bench, 'target', options.profile);
  const missing = BINARIES.filter((name) => !fs.existsSync(path.join(targetDir, exeName(name))));
  if (missing.length) {
    fail(
      `not built in ${options.profile}: ${missing.join(', ')}\n` +
        `  Run: node PortalTestBench/tools/build.mjs${options.profile === 'release' ? ' --release' : ''}`,
    );
  }

  // --- stage ---------------------------------------------------------------------------
  const platform = IS_WINDOWS ? 'windows' : IS_MACOS ? 'macos' : process.platform;
  const arch = process.arch === 'arm64' ? 'arm64' : 'x64';
  const stem = `PortalTestBench-${meta.short}-${platform}-${arch}`;

  fs.mkdirSync(options.out, { recursive: true });
  const staging = path.join(options.out, `.staging-${stem}`);
  fs.rmSync(staging, { recursive: true, force: true });

  // Staged apart, because the two platforms put it in different places: beside the executable on
  // Windows, in `Contents/Resources` inside a bundle. `resource_roots` in portal-swd knows both.
  const payload = path.join(staging, '.payload');
  fs.mkdirSync(payload, { recursive: true });
  const resources = payload;
  copyTree(path.join(bench, 'plans'), path.join(resources, 'plans'));
  const rows = stageFirmware(resources, artefacts, meta);
  step(`Staged ${rows.length} firmware images and ${fs.readdirSync(path.join(resources, 'plans')).length} plans`);

  let archiveRoot;
  if (IS_MACOS) {
    // The bundler stages the binaries, CEF, the helpers and MoltenVK, and signs inside-out. It
    // takes the resources directory rather than building one, so the two scripts have one idea
    // each of what they own.
    step('Bundling');
    run('node', [
      path.join(bench, 'tools', 'bundle-macos.mjs'),
      '--profile', options.profile,
      '--sign', options.sign,
      '--resources', resources,
      '--out', staging,
    ]);
    // The `.app` and the README travel together in a named folder rather than the bundle alone.
    // An ad-hoc signature means the first launch needs Finder -> Open, and a zip containing only
    // a bundle has nowhere to say so -- Gatekeeper's own message does not mention the way round
    // it, so an operator's first experience is a dialog with one button reading "Done".
    archiveRoot = path.join(staging, stem);
    fs.mkdirSync(archiveRoot, { recursive: true });
    fs.renameSync(path.join(staging, 'PortalTestBench.app'), path.join(archiveRoot, 'PortalTestBench.app'));
    fs.writeFileSync(path.join(archiveRoot, 'README.txt'), readme(meta, rows, platform));
    fs.rmSync(payload, { recursive: true, force: true });
  } else {
    fs.writeFileSync(path.join(staging, 'README.txt'), readme(meta, rows, platform));
    // Everything beside the executable, which is the layout `resources_dir()` looks for first.
    // The CEF payload is already there: `av-gui-cef-sys`'s build script hard-links it into
    // `target/<profile>` because `libcef.dll` is resolved through the import table before `main`
    // runs, so it cannot be staged later by the program itself.
    step('Staging binaries and the CEF payload');
    for (const entry of fs.readdirSync(targetDir, { withFileTypes: true })) {
      if (entry.isDirectory() && entry.name !== 'locales') continue;
      if (entry.name.startsWith('.')) continue;
      const from = path.join(targetDir, entry.name);
      const to = path.join(staging, entry.name);
      const keep =
        BINARIES.some((name) => entry.name === exeName(name)) ||
        /\.(dll|pak|bin|dat)$/i.test(entry.name) ||
        entry.name === 'locales';
      if (!keep) continue;
      if (entry.isDirectory()) copyTree(from, to);
      else fs.copyFileSync(from, to);
    }
    // `plans/` and `firmware/` move up beside the executables, which is where `resource_roots`
    // looks first.
    copyTree(payload, staging);
    fs.rmSync(payload, { recursive: true, force: true });
    archiveRoot = staging;
  }

  // --- zip -----------------------------------------------------------------------------
  const archive = path.join(options.out, `${stem}.zip`);
  fs.rmSync(archive, { force: true });
  step(`Compressing ${path.basename(archive)}`);
  if (IS_WINDOWS) {
    // System32's tar, explicitly: an MSYS tar on PATH reads `C:` as an rsh host specification.
    run(path.join(process.env.SystemRoot ?? 'C:\\Windows', 'System32', 'tar.exe'), [
      '-a', '-c', '-f', archive, '-C', path.dirname(archiveRoot), path.basename(archiveRoot),
    ]);
  } else {
    // `ditto` rather than `zip`: it preserves the code signature and the symlinks inside the CEF
    // framework, both of which a plain `zip -r` destroys -- and an invalidated signature costs a
    // renderer process rather than an error message.
    run('ditto', ['-c', '-k', '--sequesterRsrc', '--keepParent', archiveRoot, archive]);
  }

  fs.rmSync(staging, { recursive: true, force: true });

  console.log('');
  done(`${path.relative(root, archive)}  ${commas(fs.statSync(archive).size)} bytes`);
  if (meta.dirty) warn('Packaged from a dirty tree; README.txt and MANIFEST.md both say so.');
  console.log(`  ${rows.length} firmware images, from ${meta.short}`);
  console.log('');
  console.log('  Before handing this to anyone, run it on a machine that has never held this');
  console.log('  repository and flash one board with it. That is the only check that covers');
  console.log('  every path this script touched.');
});
