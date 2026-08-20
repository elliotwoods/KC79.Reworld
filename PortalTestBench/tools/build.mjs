// Build PortalTestBench: web bundle first, then cargo.
//
//     node tools/build.mjs [--release] [--skip-web]
//
// A port of build.ps1, which said both of these first. They are load-bearing, not preferences:
//
//   * `av_operator_app::web_assets!` resolves `web/dist` at COMPILE time, so a cargo build that
//     runs first embeds whatever the last web build left behind. The symptom is a binary that
//     starts cleanly and serves a stale page -- which reads as a host bug, not a missing build
//     step. Never run the two in parallel.
//
//   * Every cargo invocation passes an absolute `--manifest-path` AND runs with `cwd` set to the
//     app. Both, and the second is not redundant: `--manifest-path` chooses the workspace, but
//     **cargo reads `.cargo/config.toml` from the working directory upwards** and nowhere else.
//     Run from the repository root, this build picks up no `PortalTestBench/.cargo/config.toml`,
//     so on macOS the sixteen `-Wl,-U,_cef_*` allowances are absent and every binary fails at
//     link with "symbol(s) not found" -- while the identical command run from inside the app
//     succeeds. That cost a release build to find, and it only appeared once `tools/package.mjs`
//     started invoking this script from somewhere else.
//
//     The `--manifest-path` half is still needed for its own reason: there is a second complete
//     Cargo workspace behind `third_party/av-frameworks` (a link into PortalFlasher's submodule),
//     and a shell that has wandered in there builds the framework instead, successfully, with
//     nothing saying so.
//
// One thing this build has that the PowerShell one did not: `av-gui-subprocess`. It is the CEF
// helper, it is a member of this workspace rather than of a dependency, and Cargo does not build
// binary targets belonging to a dependency -- so it has to be named here or `av_gui_shell::run`
// answers `SubprocessMissing` at startup off Windows.

import fs from 'node:fs';
import path from 'node:path';

import { IS_MACOS, done, fail, main, npm, run, step, warn } from '../../tools/lib/proc.mjs';

const app = path.resolve(import.meta.dirname, '..');
const manifest = path.join(app, 'Cargo.toml');
const web = path.join(app, 'web');
const dist = path.join(web, 'dist');

/** Every binary this workspace ships. Order is cosmetic; membership is not. */
const PACKAGES = ['portal-test-bench', 'ptb', 'av-gui-subprocess'];

function parseArgs(argv) {
  const options = { release: false, skipWeb: false };
  for (const arg of argv) {
    if (arg === '--release') options.release = true;
    else if (arg === '--skip-web') options.skipWeb = true;
    else fail(`unknown argument \`${arg}\``);
  }
  return options;
}

main(() => {
  const options = parseArgs(process.argv.slice(2));

  if (!fs.existsSync(path.join(app, 'third_party', 'av-frameworks', 'crates', 'av-operator-app', 'Cargo.toml'))) {
    fail('third_party/av-frameworks is missing. Run: node tools/bootstrap.mjs');
  }

  // --- 1. web ----------------------------------------------------------------------------
  if (options.skipWeb) {
    if (!fs.existsSync(path.join(dist, 'index.html'))) {
      fail('--skip-web was given but web/dist/index.html does not exist. Build the web package once first.');
    }
    step('Skipping the web bundle (web/dist reused)');
  } else {
    step('Building the web bundle');
    npm('npm', ['run', 'build'], { cwd: web });
  }

  // --- 2. cargo --------------------------------------------------------------------------
  const profileArgs = options.release ? ['--release'] : [];
  step(`Building the Rust workspace${options.release ? ' (release)' : ''}`);

  if (IS_MACOS && !process.env.VULKAN_SDK) {
    // Not fatal -- the build succeeds and only the window fails, as `NoAdapter`, which reads as
    // "this machine has no GPU". Said here because this is the last moment before it matters.
    warn('VULKAN_SDK is unset. Source third_party/av-frameworks/tools/setup-env-macos.sh before a native run.');
  }

  const packageArgs = PACKAGES.flatMap((name) => ['-p', name]);
  run('cargo', ['build', '--manifest-path', manifest, ...packageArgs, ...profileArgs], { cwd: app });

  const target = path.join(app, 'target', options.release ? 'release' : 'debug');
  const suffix = process.platform === 'win32' ? '.exe' : '';
  console.log('');
  done('Build complete.');
  console.log(`  ${path.join(target, `portal-test-bench${suffix}`)}             # native window + http://127.0.0.1:8770`);
  console.log(`  ${path.join(target, `portal-test-bench${suffix}`)} --headless  # the same page, no window`);
  console.log(`  ${path.join(target, `portal-test-bench${suffix}`)} --simulate  # a modelled module, no hardware`);
  console.log(`  ${path.join(target, `ptb${suffix}`)} state                     # the agent's view of the same bench`);
  if (IS_MACOS) {
    console.log('');
    console.log('  A native macOS run needs a bundle -- CEF resolves its framework relative to the');
    console.log('  main bundle and cannot find it beside a bare binary:');
    console.log(`      node tools/bundle-macos.mjs${options.release ? ' --profile release' : ''}`);
  }
});
