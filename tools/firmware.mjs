// What this repository can build for an STM32G070RBT6, and how to tell a good image from a bad
// one without a board.
//
// Kept as data in one file because three other places need the same facts and would otherwise
// each restate them: `build-firmware.mjs` builds them, `package.mjs` ships them, and
// `portal-swd`'s `artefacts.rs` and `image.rs` refuse the wrong ones at flash time. The Rust
// side is the authority -- these constants mirror `portal-swd::addr` -- and `checkImage` below
// deliberately applies the *same* two refusals early, so `pio run -e no_bootloader` is caught by
// the build rather than at the moment an operator presses Flash.

import fs from 'node:fs';
import path from 'node:path';

import { commas, fail, repoRoot } from './lib/proc.mjs';

/** 128 kB of flash, split 24 kB bootloader + 104 kB application. Mirrors `portal_swd::addr`. */
export const FLASH_BASE = 0x0800_0000;
export const BOOTLOADER_BYTES = 24 * 1024;
export const APP_BASE = 0x0800_6000;
export const APP_BANK_BYTES = 104 * 1024;

/** 36 kB of SRAM at 0x20000000. The initial stack pointer must point inside it. */
const RAM_BASE = 0x2000_0000;
const RAM_BYTES = 36 * 1024;

/**
 * Every environment this repository ships, and where its output lands.
 *
 * `dir` is relative to the repository root and is *also* the path the packager mirrors into
 * `resources/firmware`, which is what lets `portal_swd::artefacts::discover_in` serve a packaged
 * copy and a developer's tree with one implementation.
 */
export const ENVIRONMENTS = [
  {
    env: 'application_bank_optical',
    project: 'PortalFW',
    label: 'PortalFW application (optical, PCB v6)',
    region: 'application',
    base: APP_BASE,
    limit: APP_BANK_BYTES,
    dir: 'PortalFW/.pio/build/application_bank_optical',
  },
  {
    env: 'application_bank_mechanical',
    project: 'PortalFW',
    label: 'PortalFW application (mechanical, PCB v4)',
    region: 'application',
    base: APP_BASE,
    limit: APP_BANK_BYTES,
    dir: 'PortalFW/.pio/build/application_bank_mechanical',
  },
  {
    env: 'bootloader',
    project: 'PortalBootloader',
    label: 'PortalBootloader (built)',
    region: 'bootloader',
    base: FLASH_BASE,
    limit: BOOTLOADER_BYTES,
    dir: 'PortalBootloader/.pio/build/bootloader',
  },
];

/**
 * The environments that must never be built into a distributable, and why.
 *
 * The same list `portal-swd`'s `REFUSED_APPLICATION_ENVS` carries, restated here because this is
 * the earlier of the two gates and a build that produced one of these would otherwise sit in
 * `.pio/build` waiting to be picked up by hand. `checkImage` catches them again by reset vector,
 * so a rename does not defeat the refusal.
 */
export const REFUSED = {
  no_bootloader:
    'links at 0x08000000, so it programs and verifies cleanly into the application slot and never runs',
  debug_no_bootloader:
    'links at 0x08000000, and is a debug build that does not fit alongside a bootloader',
  application_bank_optical_bringup:
    'suppresses Routines::startup(), so a board flashed with it never homes on its own',
};

export function environmentNamed(name) {
  const found = ENVIRONMENTS.find((e) => e.env === name);
  if (found) return found;
  if (name in REFUSED) {
    fail(`refusing to build \`${name}\`: ${REFUSED[name]}`);
  }
  fail(
    `unknown environment \`${name}\`. Known: ${ENVIRONMENTS.map((e) => e.env).join(', ')}`,
  );
}

/** `<repo>/PortalFW/.pio/build/<env>/firmware.bin`, and the ELF beside it. */
export function artefactPaths(environment, root = repoRoot()) {
  const dir = path.join(root, environment.dir);
  return { dir, bin: path.join(dir, 'firmware.bin'), elf: path.join(dir, 'firmware.elf') };
}

/**
 * Read a Cortex-M vector table's first two words.
 *
 * Word 0 is the initial stack pointer, word 1 the reset vector with the Thumb bit set. Both are
 * little-endian, and both are facts about where the image was *linked* -- which is the only thing
 * that distinguishes an application built for the bank from one built for offset zero, since the
 * two are otherwise byte-for-byte plausible.
 */
export function vectorTable(bytes) {
  if (bytes.length < 8) return null;
  return { stackPointer: bytes.readUInt32LE(0), resetVector: bytes.readUInt32LE(4) };
}

/**
 * Everything that can be known about an image without a board, as a list of complaints.
 *
 * An empty list means it is flashable. Each entry is a sentence an operator can act on rather
 * than a code, because this output is read by whoever ran the build and by nobody else.
 */
export function checkImage(environment, bytes) {
  const faults = [];

  if (bytes.length === 0) {
    faults.push('the image is empty');
    return faults;
  }
  if (bytes.length > environment.limit) {
    faults.push(
      `${commas(bytes.length)} bytes will not fit the ${commas(environment.limit)}-byte ` +
        `${environment.region} bank`,
    );
  }

  const vectors = vectorTable(bytes);
  if (!vectors) {
    faults.push('too short to hold a vector table');
    return faults;
  }

  // The mistake that costs a bench session, caught here rather than at Flash: an application
  // linked at 0x08000000 programs cleanly into the application slot, verifies cleanly, and never
  // runs. `portal-swd` refuses it too; one check is a policy and two are a guarantee.
  const { resetVector, stackPointer } = vectors;
  const top = environment.base + environment.limit;
  if (resetVector < environment.base || resetVector >= top) {
    faults.push(
      `the reset vector is 0x${resetVector.toString(16).padStart(8, '0')}, outside the ` +
        `${environment.region} bank at 0x${environment.base.toString(16).padStart(8, '0')} -- ` +
        'this image was linked for the wrong slot and would program, verify and never run',
    );
  } else if ((resetVector & 1) === 0) {
    faults.push('the reset vector has no Thumb bit set, so it is not a Cortex-M entry point');
  }

  if (stackPointer < RAM_BASE || stackPointer > RAM_BASE + RAM_BYTES) {
    faults.push(
      `the initial stack pointer is 0x${stackPointer.toString(16).padStart(8, '0')}, outside ` +
        'this part’s 36 kB of SRAM',
    );
  }

  return faults;
}

/** Read and check one built artefact. Throws with every complaint at once, not just the first. */
export function verifyBuilt(environment, root = repoRoot()) {
  const { bin, elf } = artefactPaths(environment, root);
  if (!fs.existsSync(bin)) {
    fail(`${environment.env}: no firmware.bin at ${bin}`);
  }
  const bytes = fs.readFileSync(bin);
  const faults = checkImage(environment, bytes);
  if (faults.length) {
    fail(`${environment.env} produced an unusable image:\n  - ${faults.join('\n  - ')}`);
  }
  return {
    ...environment,
    bin,
    elf: fs.existsSync(elf) ? elf : null,
    bytes: bytes.length,
    ...vectorTable(bytes),
  };
}
