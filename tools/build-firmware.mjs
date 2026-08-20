// Build the firmware this repository flashes, on either platform.
//
//     node tools/build-firmware.mjs                    # all three shipped environments
//     node tools/build-firmware.mjs --env bootloader   # one of them
//     node tools/build-firmware.mjs --clean            # `pio run -t clean` first
//     node tools/build-firmware.mjs --list             # what is available, and what is refused
//
// ## Why this exists
//
// Until now nothing in this repository built firmware. `pio run` appeared only in documentation
// and in one hint string that `portal-swd` prints when an artefact is missing -- so the two
// PlatformIO projects were built by hand, in an IDE, on one person's machine, and the flashing
// rig discovered whatever that had left behind.
//
// ## Two things it does that `pio run` does not
//
// 1. **Finds PlatformIO.** `pio` is on nobody's PATH by default; the installer puts it in a
//    virtualenv under the home directory, at a different path per platform.
// 2. **Checks what came out.** Every image is read back and its vector table examined before this
//    script reports success -- see `firmware.mjs`'s `checkImage`. An application accidentally
//    linked at 0x08000000 is caught here, at the build, rather than surviving all the way to a
//    board that programs and verifies perfectly and then does nothing.

import fs from 'node:fs';
import path from 'node:path';

import {
  IS_WINDOWS,
  commas,
  done,
  fail,
  main,
  repoRoot,
  run,
  step,
  warn,
  which,
} from './lib/proc.mjs';
import { ENVIRONMENTS, REFUSED, artefactPaths, environmentNamed, verifyBuilt } from './firmware.mjs';

/**
 * PlatformIO, wherever the installer put it.
 *
 * PATH first, so a developer who has arranged their own is not overridden. Then the standard
 * virtualenv, whose location differs by platform -- `penv/bin` against `penv/Scripts`, and the
 * `.exe` suffix. This machine is the ordinary case: `pio` is not on PATH and
 * `~/.platformio/penv/bin/pio` is.
 */
export function findPio() {
  const onPath = which('pio');
  if (onPath) return onPath;

  const home = process.env.HOME ?? process.env.USERPROFILE;
  if (home) {
    const candidate = IS_WINDOWS
      ? path.join(home, '.platformio', 'penv', 'Scripts', 'pio.exe')
      : path.join(home, '.platformio', 'penv', 'bin', 'pio');
    if (fs.existsSync(candidate)) return candidate;
  }

  fail(
    'PlatformIO not found.\n' +
      '  Looked on PATH and in ~/.platformio/penv.\n' +
      '  Install it with:  pip install -U platformio\n' +
      '  or from VS Code:  the PlatformIO IDE extension.\n' +
      '\n' +
      '  Building firmware needs it. Flashing artefacts that are already built -- including the\n' +
      '  ones inside a packaged PortalTestBench -- does not.',
  );
  return null;
}

function parseArgs(argv) {
  const options = { envs: [], clean: false, list: false };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--env' || arg === '-e') {
      const name = argv[i + 1];
      if (!name) fail('--env needs an environment name');
      options.envs.push(name);
      i += 1;
    } else if (arg === '--clean') {
      options.clean = true;
    } else if (arg === '--list') {
      options.list = true;
    } else {
      fail(`unknown argument \`${arg}\`. Try --list.`);
    }
  }
  return options;
}

function list() {
  console.log('\nShipped environments:\n');
  for (const environment of ENVIRONMENTS) {
    const { bin } = artefactPaths(environment);
    const built = fs.existsSync(bin) ? `${commas(fs.statSync(bin).size)} bytes` : 'not built';
    console.log(
      `  ${environment.env.padEnd(30)} 0x${environment.base.toString(16)}  ` +
        `${environment.label}\n${' '.repeat(32)}${built}`,
    );
  }
  console.log('\nRefused, and why:\n');
  for (const [name, reason] of Object.entries(REFUSED)) {
    console.log(`  ${name.padEnd(30)} ${reason}`);
  }
  console.log('');
}

main(() => {
  const options = parseArgs(process.argv.slice(2));
  if (options.list) {
    list();
    return;
  }

  const root = repoRoot();
  const selected = options.envs.length
    ? options.envs.map(environmentNamed)
    : ENVIRONMENTS;

  const pio = findPio();
  step(`PlatformIO at ${pio}`);

  const built = [];
  for (const environment of selected) {
    const cwd = path.join(root, environment.project);
    if (!fs.existsSync(path.join(cwd, 'platformio.ini'))) {
      fail(`${environment.project} has no platformio.ini at ${cwd}`);
    }

    if (options.clean) {
      step(`${environment.env}: clean`);
      run(pio, ['run', '-e', environment.env, '-t', 'clean'], { cwd });
    }

    step(`${environment.env}: build (${environment.project})`);
    run(pio, ['run', '-e', environment.env], { cwd });
    built.push(verifyBuilt(environment, root));
  }

  console.log('');
  step('Checked every image against its bank and its vector table:');
  for (const artefact of built) {
    const headroom = artefact.limit - artefact.bytes;
    console.log(
      `  ${artefact.env.padEnd(30)} ${commas(artefact.bytes).padStart(8)} bytes  ` +
        `(${commas(headroom)} free)  reset 0x${artefact.resetVector.toString(16)}` +
        `${artefact.elf ? '' : '   no .elf beside it'}`,
    );
  }

  const withoutElf = built.filter((a) => !a.elf);
  if (withoutElf.length) {
    warn(
      'Some builds produced no .elf. The flasher resolves its liveness symbol from one, so a\n' +
        '    run check will be unavailable for those images.',
    );
  }

  console.log('');
  done(`${built.length} image(s) built and checked.`);
  console.log('  Next:  node tools/package.mjs      # wrap them up with the bench');
});
