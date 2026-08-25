// Running other programs, on both platforms, without the two traps that make it look easy.
//
// ## Trap one: `.cmd` on Windows
//
// `npm` and `npx` are batch files there, and since Node 18.20 `spawnSync` refuses to execute one
// without a shell (CVE-2024-27980). A script that works on macOS and fails on Windows with
// `EINVAL` and no useful message is the ordinary outcome of not knowing this. `npm()` below runs
// through the shell on Windows and directly everywhere else; its arguments are always simple
// words, which is what makes that safe.
//
// `cargo` and `git` are real executables, so they never take the shell, and their arguments --
// which include absolute paths that routinely contain spaces on Windows -- are passed through
// verbatim.
//
// ## Trap two: an exit code nobody read
//
// Every helper here throws on a non-zero exit. `tryRun` is the only way to get one back as data,
// and it says so in its name.

import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

export const IS_WINDOWS = process.platform === 'win32';
export const IS_MACOS = process.platform === 'darwin';

// Built rather than typed, so no escape byte ever sits literally in this file.
const ESC = String.fromCharCode(27);
const CYAN = `${ESC}[36m`;
const YELLOW = `${ESC}[33m`;
const GREEN = `${ESC}[32m`;
const RESET = `${ESC}[0m`;

export function step(message) {
  console.log(`${CYAN}==> ${message}${RESET}`);
}

export function warn(message) {
  console.log(`${YELLOW}!!  ${message}${RESET}`);
}

export function done(message) {
  console.log(`${GREEN}${message}${RESET}`);
}

/** A failure the caller is expected to print and exit on, rather than a stack trace. */
export class BuildError extends Error {}

export function fail(message) {
  throw new BuildError(message);
}

/** Run to completion, inheriting stdio. Throws unless it exits 0. */
export function run(cmd, args, opts = {}) {
  const result = spawnSync(cmd, args, { stdio: 'inherit', ...opts });
  if (result.error) fail(`${cmd}: ${result.error.message}`);
  if (result.status !== 0) {
    fail(`${cmd} ${args.join(' ')} failed with exit code ${result.status}`);
  }
}

/** Run and hand back the outcome. The only way to treat a non-zero exit as data. */
export function tryRun(cmd, args, opts = {}) {
  const result = spawnSync(cmd, args, { encoding: 'utf8', ...opts });
  return {
    status: result.status,
    stdout: (result.stdout ?? '').trim(),
    stderr: (result.stderr ?? '').trim(),
    ok: result.status === 0,
  };
}

/** `npm` / `npx`, through the shell on Windows only. See the trap note above. */
export function npm(tool, args, opts = {}) {
  run(tool, args, { shell: IS_WINDOWS, ...opts });
}

/** The first executable of that name on PATH, or `null`. */
export function which(cmd) {
  const probe = IS_WINDOWS ? 'where' : 'which';
  const result = tryRun(probe, [cmd]);
  if (!result.ok) return null;
  const first = result.stdout.split(/\r?\n/)[0].trim();
  return first.length ? first : null;
}

/** `portal-test-bench` -> `portal-test-bench.exe` on Windows, unchanged elsewhere. */
export function exeName(name) {
  return IS_WINDOWS ? `${name}.exe` : name;
}

/** The repository root, from this file's location rather than from the working directory. */
export function repoRoot() {
  return path.resolve(import.meta.dirname, '..', '..');
}

export function sizeOf(file) {
  return fs.statSync(file).size;
}

/** `1234567` -> `1,234,567`. Byte counts are compared by eye against a bank limit. */
export function commas(n) {
  return n.toLocaleString('en-US');
}

/** Print a `BuildError` as its message and anything else as itself, then exit non-zero. */
export function main(fn) {
  try {
    fn();
  } catch (error) {
    if (error instanceof BuildError) {
      console.error(`\n${YELLOW}${error.message}${RESET}`);
      process.exit(1);
    }
    throw error;
  }
}
