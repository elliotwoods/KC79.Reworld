// Idempotent one-time setup for PortalTestBench, on Windows or macOS.
//
//     node tools/bootstrap.mjs [--framework <path>] [--skip-web] [--skip-cef]
//
// A port of bootstrap.ps1, which said all of this first and in PowerShell. The jobs, in order:
//
//  1. Point `third_party/av-frameworks` at a framework checkout. This app deliberately does NOT
//     carry its own submodule: PortalFlasher already pins one in the same repository, and a
//     second checkout would be several hundred megabytes AND a second pinned revision that could
//     drift from PortalFlasher's without anything saying so. A directory junction on Windows, a
//     symlink on macOS -- explicitly permitted by the framework's operator-app-starter.md ("teams
//     that maintain one sibling clone for several projects MAY replace that directory with a
//     local link"). Cargo path dependencies, npm's `file:` dependency and check-av-app.ps1 all
//     resolve through it unchanged.
//
//  2. Record/verify `framework.lock`. The link means the framework revision is decided by
//     PortalFlasher. That is fine, but it must not be *silent*.
//
//  3. `npm install` for the web package, run from INSIDE web/. Not `npm --prefix web install`:
//     for `install` (as opposed to `run`) npm resolves the `file:` dependency relative to the
//     process cwd, not the prefix, and silently produces a broken tree.
//
//  4. On macOS only, fetch CEF. This is the step that has no Windows counterpart and is easy to
//     read as optional. It is not: off Windows this application opens a **composed** window (see
//     `av-app.toml`), `av-gui-shell` therefore compiles `av-gui-cef-sys`, and that crate's
//     build.rs *panics* without `vendor/cef`. Roughly 124 MB downloaded, ~574 MB unpacked, once.
//
// Everything here is safe to re-run.

import fs from 'node:fs';
import path from 'node:path';

import {
  IS_MACOS,
  IS_WINDOWS,
  done,
  fail,
  main,
  npm,
  run,
  step,
  tryRun,
  warn,
  which,
} from '../../tools/lib/proc.mjs';

const app = path.resolve(import.meta.dirname, '..');
const repo = path.resolve(app, '..');
const link = path.join(app, 'third_party', 'av-frameworks');
const lockFile = path.join(app, 'framework.lock');

function parseArgs(argv) {
  const options = { framework: null, skipWeb: false, skipCef: false };
  for (let i = 0; i < argv.length; i += 1) {
    if (argv[i] === '--framework') {
      options.framework = argv[i + 1];
      if (!options.framework) fail('--framework needs a path');
      i += 1;
    } else if (argv[i] === '--skip-web') {
      options.skipWeb = true;
    } else if (argv[i] === '--skip-cef') {
      options.skipCef = true;
    } else {
      fail(`unknown argument \`${argv[i]}\``);
    }
  }
  return options;
}

function isFrameworkCheckout(dir) {
  return dir && fs.existsSync(path.join(dir, 'crates', 'av-operator-app', 'Cargo.toml'));
}

/** Where the framework is, in the order the previous script looked. */
function resolveFramework(requested) {
  if (requested) {
    if (!isFrameworkCheckout(requested)) fail(`${requested} is not an av-frameworks checkout`);
    return path.resolve(requested);
  }

  const submodule = path.join(repo, 'PortalFlasher', 'third_party', 'av-frameworks');
  if (isFrameworkCheckout(submodule)) return submodule;

  const sibling = path.join(path.dirname(repo), 'av-frameworks');
  if (isFrameworkCheckout(sibling)) {
    warn(`PortalFlasher's submodule is not initialised; falling back to ${sibling}`);
    return sibling;
  }

  // Named explicitly, and that matters: the repository root carries a `fonts` gitlink with no
  // `.gitmodules` entry, so a bare `git submodule update --init --recursive` fails before it
  // reaches this one.
  fail(
    'No usable av-frameworks checkout found.\n' +
      `\n  Tried: ${submodule}\n         ${sibling}\n` +
      '\n  Initialise PortalFlasher\'s submodule by name -- a bare `git submodule update --init' +
      '\n  --recursive` at the repository root fails on the broken \'fonts\' gitlink:\n' +
      `\n      git -C "${repo}" submodule update --init PortalFlasher/third_party/av-frameworks\n`,
  );
  return null;
}

/**
 * Make `third_party/av-frameworks` point at `target`.
 *
 * A junction on Windows via `mklink /J` -- not `New-Item -ItemType Junction`, and not a symlink,
 * because a symlink there needs Developer Mode or an elevated shell and a junction does not. A
 * plain directory symlink on macOS, where none of that applies.
 */
function linkFramework(target) {
  fs.mkdirSync(path.dirname(link), { recursive: true });

  const existing = fs.existsSync(link) ? fs.lstatSync(link) : null;
  if (existing) {
    const isLink = existing.isSymbolicLink() || IS_WINDOWS;
    if (!isLink && existing.isDirectory()) {
      fail(
        `${link} exists and is a real directory, not a link. Remove it and re-run; this app must` +
          ' not own a second framework checkout.',
      );
    }
    const current = fs.realpathSync(link);
    if (current === fs.realpathSync(target)) {
      step('Framework link already present');
      return;
    }
    warn(`Re-pointing the link to ${target}`);
    fs.rmSync(link, { recursive: true, force: true });
  }

  step(`Linking third_party/av-frameworks -> ${target}`);
  if (IS_WINDOWS) {
    run('cmd', ['/c', 'mklink', '/J', link, target], { stdio: 'ignore' });
  } else {
    fs.symlinkSync(target, link, 'dir');
  }
}

/** Record the revision this app was bootstrapped against, or say loudly that it has moved. */
function checkLock(framework) {
  const head = tryRun('git', ['-C', framework, 'rev-parse', 'HEAD']);
  if (!head.ok) {
    warn('Could not read the framework revision; skipping the lock check.');
    return;
  }
  if (!fs.existsSync(lockFile)) {
    fs.writeFileSync(lockFile, `${head.stdout}\n`);
    step(`Recorded framework revision ${head.stdout.slice(0, 10)} in framework.lock`);
    return;
  }
  const recorded = fs.readFileSync(lockFile, 'utf8').trim();
  if (recorded === head.stdout) {
    step(`Framework at ${head.stdout.slice(0, 10)} (matches framework.lock)`);
  } else {
    warn(
      'The framework checkout has moved since this app was bootstrapped.\n' +
        `      recorded: ${recorded}\n` +
        `      actual:   ${head.stdout}\n` +
        '    This is shared with PortalFlasher. Re-run the full test suite before trusting a\n' +
        '    build, and update framework.lock deliberately once you have.',
    );
  }
}

/**
 * CEF, which off Windows is a build prerequisite rather than a runtime payload.
 *
 * `av-gui-cef-sys/build.rs` panics on macOS when `vendor/cef` is absent, so without this the
 * first `cargo build` fails several minutes in with a message about a PowerShell script.
 */
function fetchCef(framework) {
  const vendored = path.join(
    framework,
    'vendor',
    'cef',
    'Release',
    'Chromium Embedded Framework.framework',
  );
  if (fs.existsSync(vendored)) {
    step('CEF already vendored');
    return;
  }
  step('Fetching CEF (~124 MB download, ~574 MB unpacked, once)');
  run('node', [path.join(framework, 'tools', 'fetch-cef.mjs')], { cwd: framework });
}

/**
 * The Vulkan SDK, which a composed window composites through on macOS.
 *
 * Advisory rather than fatal, because the Rust build succeeds without it and only the GPU path
 * fails -- but it fails as `NoAdapter`, which reads as "this machine has no GPU" rather than as a
 * missing environment variable, and that is the single most confusing failure in a fresh macOS
 * checkout. Say it here, once, while there is context.
 */
function checkVulkan(framework) {
  if (process.env.VULKAN_SDK) {
    step(`Vulkan SDK at ${process.env.VULKAN_SDK}`);
    return;
  }
  const home = process.env.HOME;
  const root = home ? path.join(home, 'VulkanSDK') : null;
  if (root && fs.existsSync(root) && fs.readdirSync(root).length) {
    warn(
      'A Vulkan SDK is installed but VULKAN_SDK is unset. Before building or running:\n' +
        `      . ${path.join(framework, 'tools', 'setup-env-macos.sh')}`,
    );
  } else {
    warn(
      'No Vulkan SDK under ~/VulkanSDK. The macOS window composites through MoltenVK, so the\n' +
        '    native window will report NoAdapter without it. Install the LunarG macOS SDK, then:\n' +
        `      . ${path.join(framework, 'tools', 'setup-env-macos.sh')}\n` +
        '    `--headless` needs none of it.',
    );
  }
}

main(() => {
  const options = parseArgs(process.argv.slice(2));

  const framework = resolveFramework(options.framework);
  linkFramework(framework);
  checkLock(framework);

  if (!options.skipWeb) {
    const web = path.join(app, 'web');
    step('npm install (from inside web/)');
    const hasLock = fs.existsSync(path.join(web, 'package-lock.json'));
    npm('npm', [hasLock ? 'ci' : 'install'], { cwd: web });
  }

  if (IS_MACOS && !options.skipCef) {
    fetchCef(framework);
    checkVulkan(framework);
  }

  // Advisory: firmware is built by `tools/build-firmware.mjs` at the repository root, and a bench
  // that only flashes what a package already carries never needs PlatformIO at all.
  if (!which('pio')) {
    const home = process.env.HOME ?? process.env.USERPROFILE;
    const penv = home
      ? path.join(home, '.platformio', 'penv', IS_WINDOWS ? 'Scripts' : 'bin', IS_WINDOWS ? 'pio.exe' : 'pio')
      : null;
    if (penv && fs.existsSync(penv)) {
      step(`PlatformIO at ${penv}`);
    } else {
      warn(
        'PlatformIO not found. `node ../tools/build-firmware.mjs` will not work; flashing\n' +
          '    prebuilt artefacts still will.',
      );
    }
  }

  console.log('');
  done('Bootstrap complete.');
  console.log('  Next:  node tools/build.mjs');
});
