// Every gate that has to be green before claiming PortalTestBench works.
//
//     node tools/test.mjs [--fast]
//
// A port of test.ps1, keeping its order (fail-fastest-first, so a broken build is reported in
// seconds rather than minutes) and its two hazards:
//
//   * Gate 4 (clippy) is SCOPED to this workspace's own packages, deliberately. An unscoped
//     `cargo clippy --all` or `cargo fmt --all` reaches through the third_party link and rewrites
//     the pinned framework -- and because that link points at PortalFlasher's submodule, it
//     dirties PortalFlasher's checkout too. Gate 7 exists to catch it if it happens anyway.
//
//   * Gate 6 is the one that matters most for this product: it exercises the agent's own path
//     (ptb -> engine -> verdict -> NDJSON) end to end with no probe, no serial port and no board.
//
// One gate is Windows-only and says so rather than being quietly dropped: `check-av-app.ps1`
// belongs to the pinned framework submodule, is PowerShell, and is not ours to port. A macOS run
// prints what it skipped, because a gate nobody can see is not a gate.

import fs from 'node:fs';
import path from 'node:path';

import {
  IS_WINDOWS,
  done,
  exeName,
  fail,
  main,
  npm,
  run,
  step,
  tryRun,
  warn,
} from '../../tools/lib/proc.mjs';

const app = path.resolve(import.meta.dirname, '..');
const manifest = path.join(app, 'Cargo.toml');
const web = path.join(app, 'web');
const framework = path.join(app, 'third_party', 'av-frameworks');

let gate = 0;
function startGate(message) {
  gate += 1;
  console.log('');
  step(`[${gate}] ${message}`);
}

/**
 * The agent's own path, with no probe, no serial port and no board.
 *
 * A port of e2e-sim.ps1, whose careful note about not redirecting native stderr in PowerShell 5.1
 * has no counterpart here -- which is one small reason this is Node now.
 */
function e2eSim() {
  const ptb = path.join(app, 'target', 'debug', exeName('ptb'));
  if (!fs.existsSync(ptb)) {
    fail(`ptb not found at ${ptb}. Run: node tools/build.mjs`);
  }

  const version = tryRun(ptb, ['version']);
  if (!version.ok) fail(`ptb version exited ${version.status}`);

  let parsed;
  try {
    parsed = JSON.parse(version.stdout);
  } catch {
    fail(`ptb version did not print JSON: ${version.stdout}`);
  }
  // The profile string is written into every session file and is how a reader years later knows
  // what shape of NDJSON they are holding. If it changes, it changes deliberately.
  if (parsed.report_profile !== 'bench/1') {
    fail(`report profile is '${parsed.report_profile}', expected 'bench/1'`);
  }
  console.log(`    ptb version ok -- report profile ${parsed.report_profile}`);

  // A command that is not wired yet must fail loudly rather than print an empty document that
  // would read like an answer.
  const state = tryRun(ptb, ['state']);
  if (state.ok) {
    fail('ptb state exited 0 but the bench worker does not exist yet -- it must not fake success');
  }
  console.log('    ptb state correctly refuses rather than printing an empty answer');

  console.log('');
  warn('NOT YET COVERED (lands with the engine, M3):');
  warn('  - run a plan to a pass verdict and assert the NDJSON');
  warn('  - a failing criterion exits 1 and names itself');
  warn('  - abort exits 2 and records an escape');
}

main(() => {
  const argv = process.argv.slice(2);
  const fast = argv.includes('--fast');

  // `tools/e2e-sim.ps1` still exists and still runs exactly this one gate, because it is named
  // from elsewhere. Running it through here rather than duplicating it is the whole reason the
  // PowerShell scripts became wrappers.
  if (argv.includes('--only-e2e')) {
    startGate('simulated end-to-end (the agent path, with no hardware)');
    e2eSim();
    return;
  }

  startGate('cargo test (engine, verdicts, plan validation, transports, protocol goldens)');
  // `cwd` as well as `--manifest-path`: cargo reads `.cargo/config.toml` from the working
  // directory upwards, so a run started from anywhere else silently drops this workspace's macOS
  // link flags. See the note at the top of build.mjs.
  run('cargo', ['test', '--manifest-path', manifest, '--workspace'], { cwd: app });

  startGate('vitest (the pure page models)');
  npm('npx', ['vitest', 'run'], { cwd: web });

  startGate('tsc + vite build');
  npm('npm', ['run', 'build'], { cwd: web });

  if (fast) {
    console.log('');
    done('Fast gates passed (1-3). Run without --fast before claiming anything works.');
    return;
  }

  startGate('clippy (scoped -- never --all, see the note above)');
  run('cargo', [
    'clippy',
    '--manifest-path',
    manifest,
    '-p', 'bench-core',
    '-p', 'portal-test-bench',
    '-p', 'ptb',
    '-p', 'av-gui-subprocess',
    '--all-targets',
    '--all-features',
    '--',
    '-D', 'warnings',
  ], { cwd: app });

  startGate('check-av-app (the framework application contract)');
  if (IS_WINDOWS) {
    run('powershell', [
      '-NoProfile',
      '-ExecutionPolicy', 'Bypass',
      '-File', path.join(framework, 'tools', 'check-av-app.ps1'),
      '-AppPath', app,
    ]);
  } else {
    // Announced rather than silently skipped, the same way the framework's own portable workflow
    // announces its GPU gate. "The tests passed" must not be readable as "the contract holds".
    warn('SKIPPED off Windows: check-av-app.ps1 is PowerShell and belongs to the pinned submodule.');
    warn('  Run this gate on Windows before releasing a package.');
  }

  startGate('simulated end-to-end (the agent path, with no hardware)');
  e2eSim();

  startGate('framework checkout still clean');
  const dirty = tryRun('git', ['-C', framework, 'status', '--porcelain']);
  if (!dirty.ok) {
    warn('(could not read framework git status; skipping)');
  } else if (dirty.stdout) {
    fail(
      `The pinned framework checkout is dirty:\n${dirty.stdout}\n` +
        'Something ran an unscoped fmt/clippy/fix. This link is PortalFlasher\'s submodule, so its\n' +
        'checkout is dirty too. Revert it there before committing anything.',
    );
  }

  console.log('');
  done('All gates passed.');
});
