// What this repository can build for an STM32G070RBT6, and how to tell a good image from a bad
// one without a board.
//
// Kept as data in one file because three other places need the same facts and would otherwise
// each restate them: `build-firmware.mjs` builds them, `package.mjs` ships them, and
// `portal-swd`'s `artefacts.rs` and `image.rs` refuse the wrong ones at flash time. `checkImage`
// below deliberately applies the *same* refusals early, so `pio run -e no_bootloader` is caught by
// the build rather than at the moment an operator presses Flash.
//
// The addresses themselves are not restated here at all: they are read out of
// `PortalBootloader/include/portal_flash_layout.h`, which is the definition every reader shares.
// That file is written as bare `#define`s with hexadecimal literals precisely so that a reader
// which is not a C compiler -- this one, `PortalFW/layout_check.py`, and two Rust modules -- can
// parse it as text rather than copy it and drift.

import fs from 'node:fs';
import path from 'node:path';

import { commas, fail, repoRoot } from './lib/proc.mjs';

/**
 * Every `#define NAME 0x...` in the firmware's layout header, as a map.
 *
 * Only bare hexadecimal literals are understood, which is the rule the header sets for itself. A
 * `#define` that grew an expression is not silently mis-evaluated -- it simply does not appear,
 * and the lookup below then fails by name.
 */
function readLayout() {
  const file = path.join(repoRoot(), 'PortalBootloader/include/portal_flash_layout.h');
  let text;
  try {
    text = fs.readFileSync(file, 'utf8');
  } catch (error) {
    fail(`cannot read the flash layout at ${file}: ${error.message}`);
  }
  const values = new Map();
  for (const line of text.split('\n')) {
    const match = /^#define\s+(PORTAL_\w+)\s+(0x[0-9A-Fa-f]+)/.exec(line);
    if (match) values.set(match[1], Number.parseInt(match[2], 16));
  }
  return (name) => {
    const value = values.get(name);
    // Loud, and at import time: a missing name means the header moved or was rewritten, and
    // every size and address check below would otherwise quietly compare against `undefined`.
    if (value === undefined) {
      fail(`\`${name}\` is not defined as a bare hex literal in ${file}`);
    }
    return value;
  };
}

const define = readLayout();

/** 128 kB of flash. The bootloader starts here; where the application starts depends on which. */
export const FLASH_BASE = define('PORTAL_FLASH_BASE');
export const FLASH_PAGE_BYTES = define('PORTAL_FLASH_PAGE_BYTES');
/** The v6 bootloader bank, and what v4/v5 occupied. Both are in the field. */
export const BOOTLOADER_BYTES = define('PORTAL_BOOTLOADER_BYTES');
export const BOOTLOADER_BYTES_LEGACY = define('PORTAL_BOOTLOADER_BYTES_LEGACY');
/** The two application bases: under a v6 bootloader, and under a v4/v5 one. */
export const APP_BASE = define('PORTAL_APP_BASE');
export const APP_BASE_LEGACY = define('PORTAL_APP_BASE_LEGACY');
/** One past the last application byte: the first of the three durable pages. */
export const APP_END = define('PORTAL_APP_END');

/** The descriptor an application image carries, stating which bank it was linked for. */
export const APP_DESCRIPTOR_OFFSET = define('PORTAL_APP_DESCRIPTOR_OFFSET');
export const APP_DESCRIPTOR_BYTES = define('PORTAL_APP_DESCRIPTOR_BYTES');
export const APP_VERSION_BYTES = define('PORTAL_APP_VERSION_BYTES');
const APP_DESCRIPTOR_MAGIC = 'KC79APP1';

/** 36 kB of SRAM at 0x20000000. The initial stack pointer must point inside it. */
const RAM_BASE = define('PORTAL_RAM_BASE');
const RAM_BYTES = define('PORTAL_RAM_END') - RAM_BASE;

/**
 * Every environment this repository ships, and where its output lands.
 *
 * Two PCB revisions times two bootloader generations, plus the bootloader itself. The
 * `*_legacy_base` pair links at `0x08006000` for a board whose bootloader has not been replaced
 * yet; both pairs are current, because a fielded fleet contains both.
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
    limit: APP_END - APP_BASE,
    dir: 'PortalFW/.pio/build/application_bank_optical',
  },
  {
    env: 'application_bank_mechanical',
    project: 'PortalFW',
    label: 'PortalFW application (mechanical, PCB v4)',
    region: 'application',
    base: APP_BASE,
    limit: APP_END - APP_BASE,
    dir: 'PortalFW/.pio/build/application_bank_mechanical',
  },
  {
    env: 'application_bank_optical_legacy_base',
    project: 'PortalFW',
    label: 'PortalFW application (optical, PCB v6) for boards still on bootloader v4/v5',
    region: 'application',
    base: APP_BASE_LEGACY,
    limit: APP_END - APP_BASE_LEGACY,
    dir: 'PortalFW/.pio/build/application_bank_optical_legacy_base',
  },
  {
    env: 'application_bank_mechanical_legacy_base',
    project: 'PortalFW',
    label: 'PortalFW application (mechanical, PCB v4) for boards still on bootloader v4/v5',
    region: 'application',
    base: APP_BASE_LEGACY,
    limit: APP_END - APP_BASE_LEGACY,
    dir: 'PortalFW/.pio/build/application_bank_mechanical_legacy_base',
  },
  {
    env: 'bootloader',
    project: 'PortalBootloader',
    label: 'PortalBootloader (built)',
    region: 'bootloader',
    base: FLASH_BASE,
    // What this repository builds. An image that identifies itself as older is measured against
    // the larger bank instead -- see `limitFor`.
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
 * The descriptor an application image carries at `base + 0xC0`, or `null` if it has none.
 *
 * This is the only thing in an image that says which bank it was built for. An application linked
 * for `0x08004000` and one linked for `0x08006000` are otherwise byte-for-byte plausible: stack
 * pointer in SRAM, Thumb reset vector inside the application bank, then code. The banks overlap,
 * so even the reset vector cannot separate them -- and with both builds sitting in `.pio/build`
 * under names differing by a suffix, the wrong one would program, verify, and hard-fault at some
 * later address unrelated to the mistake.
 *
 * `null` covers both "built before the descriptor existed" and "the magic does not match", which
 * are deliberately the same answer: a damaged descriptor must never be read as a valid one.
 */
export function readDescriptor(bytes) {
  const at = APP_DESCRIPTOR_OFFSET;
  if (bytes.length < at + APP_DESCRIPTOR_BYTES) return null;
  if (bytes.toString('latin1', at, at + 8) !== APP_DESCRIPTOR_MAGIC) return null;
  const version = bytes.subarray(at + 16, at + 16 + APP_VERSION_BYTES);
  const end = version.indexOf(0);
  return {
    app_base: bytes.readUInt32LE(at + 8),
    flags: bytes.readUInt32LE(at + 12),
    version: version.toString('latin1', 0, end === -1 ? version.length : end),
  };
}

/**
 * The bootloader's major version, scraped from its `Bootloader v…` banner, or `null`.
 *
 * A plain string literal in the image, so this needs no symbols. The number decides which bank the
 * image is held to, which is why an image whose banner cannot be read is held to the *larger* one:
 * assuming an unidentifiable bootloader is the newest would fail the committed reference image on
 * a rule that was never written for it.
 */
export function bannerVersion(bytes) {
  const at = bytes.indexOf('Bootloader v', 0, 'latin1');
  if (at === -1) return null;
  const digits = /^\d+/.exec(bytes.toString('latin1', at + 'Bootloader v'.length, at + 20));
  return digits ? Number.parseInt(digits[0], 10) : null;
}

/**
 * How many bytes this image may occupy, given what it says about itself.
 *
 * Only the bootloader is variable: v6 shrank to 16 kB, and anything older -- including the
 * committed reference -- legitimately fills 24 kB. An application's bank follows from the base it
 * was linked for, which the environment already names.
 */
export function limitFor(environment, bytes) {
  if (environment.region !== 'bootloader') return environment.limit;
  const version = bannerVersion(bytes);
  return version !== null && version >= 6 ? BOOTLOADER_BYTES : BOOTLOADER_BYTES_LEGACY;
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
  const limit = limitFor(environment, bytes);
  if (bytes.length > limit) {
    faults.push(
      `${commas(bytes.length)} bytes will not fit the ${commas(limit)}-byte ` +
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
  const top = environment.base + limit;
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

  // The one mistake the reset vector cannot catch, because the two application banks overlap: a
  // build for the other bootloader generation. The descriptor states the base outright, so this
  // is a comparison rather than a heuristic. An image with no descriptor is left alone -- that is
  // every application built before the descriptor existed, and they are legacy-base by definition.
  if (environment.region === 'application') {
    const descriptor = readDescriptor(bytes);
    if (descriptor && descriptor.app_base !== environment.base) {
      faults.push(
        `this image is linked for the other bank: its descriptor says ` +
          `0x${descriptor.app_base.toString(16).padStart(8, '0')} and \`${environment.env}\` ` +
          `links at 0x${environment.base.toString(16).padStart(8, '0')} -- flashed as it is, it ` +
          'would program, verify, and hard-fault at the first absolute address it touches',
      );
    }
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
    // The limit the image was actually measured against, so the headroom the build prints is the
    // headroom that was checked rather than a nominal one.
    limit: limitFor(environment, bytes),
    bin,
    elf: fs.existsSync(elf) ? elf : null,
    bytes: bytes.length,
    descriptor: environment.region === 'application' ? readDescriptor(bytes) : null,
    ...vectorTable(bytes),
  };
}
