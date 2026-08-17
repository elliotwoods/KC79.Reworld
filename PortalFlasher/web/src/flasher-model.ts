/**
 * The page's pure half.
 *
 * Everything that decides what the rig *says* is a function of plain values, tested without a bus
 * and without jsdom, so the component is left with nothing but subscriptions and markup. Same
 * discipline as the framework's own `web/src/vision/devices.ts`.
 *
 * # Enum values are read by name
 *
 * Never by discriminant. The Rust side declares `/mode/*`, `/rig/phase` and `/rig/cue` as
 * enumerations with names, and a page keyed on `3` would invert silently the moment someone
 * reordered the variant list in `schema.rs`.
 */

/** Tone vocabulary shared with the framework's pills and status items. */
export type Tone = 'idle' | 'active' | 'ok' | 'warn' | 'error' | 'offline';

/** `/mode/desired`, by name. */
export type Mode = 'manual' | 'auto';

/** `/rig/phase`, by name. */
export type Phase =
  | 'disarmed'
  | 'idle'
  | 'debouncing'
  | 'flashing'
  | 'run-check'
  | 'await-removal'
  | 'probe-lost';

/** `/rig/expect`, by name. */
export type Expect = 'flash' | 'run-check';

/** `/rig/cue`, by name. */
export type Cue =
  | 'none'
  | 'armed'
  | 'disarmed'
  | 'busy'
  | 'flashed-cycle-it'
  | 'pass'
  | 'fail'
  | 'rearmed';

/** `/device/layout`, by name. */
export type Layout = 'unknown' | 'erased' | 'split' | 'flat' | 'unrecognised';

export interface Tile {
  /** Big enough to read across a bench. */
  headline: string;
  /** What the operator should physically do next. Never empty. */
  instruction: string;
  tone: Tone;
}

export interface RigState {
  mode: Mode;
  armed: boolean;
  phase: Phase;
  expect: Expect;
  lastCue: Cue;
  probeConnected: boolean;
  targetPresent: boolean;
  busy: boolean;
  hasImage: boolean;
  /** Which stage of a flash pass is running. `idle` between passes. */
  step: Step;
  /** How far through that stage, 0..1. */
  stepFraction: number;
}

export type Step =
  | 'idle'
  | 'attach'
  | 'option-bytes'
  | 'erase'
  | 'program'
  | 'readback'
  | 'reset-run';

/**
 * What to say while a pass is running, and whether the board can still be lifted.
 *
 * The second half is the reason this exists. `busy` is one bit, and it covers both the seconds
 * where lifting the board costs nothing and the seconds where it leaves a half-erased part. An
 * operator cannot be expected to know which is which by watching a spinner.
 */
export function stepSummary(
  state: RigState,
): { label: string; committed: boolean; determinate: boolean } | null {
  if (state.step === 'idle') return null;
  const percent = Math.round(state.stepFraction * 100);
  // Attach and the option-byte write both precede the erase; everything from the erase onward has
  // already changed the part, and stopping there is the state the whole removal gate exists for.
  const committed = state.step === 'erase' || state.step === 'program';
  // Erase, program and readback each move through the whole part and report as they go. Attach,
  // the option-byte write and reset-run are single events: a bar sitting at 0% during one of them
  // reads as stuck, which is exactly the wrong thing to say about the seconds before an erase.
  const determinate =
    state.step === 'erase' || state.step === 'program' || state.step === 'readback';
  return {
    label: determinate ? `${state.step} ${percent}%` : state.step,
    committed,
    determinate,
  };
}

/**
 * What the rig is doing, and what the operator should do about it.
 *
 * The instruction is the load-bearing half. An operator who is not watching the screen still
 * glances at it when a tone surprises them, and "cycle it" versus "next board" is the one thing
 * they need at that moment — the two success tones are otherwise only distinguishable by ear.
 */
export function tileFor(state: RigState): Tile {
  if (!state.probeConnected) {
    return { headline: 'No probe', instruction: 'Connect an ST-Link, then Rescan', tone: 'error' };
  }
  return state.mode === 'auto' ? autoTile(state) : manualTile(state);
}

/**
 * Manual is the default and has no armed state at all — it is a person pressing a button, so the
 * tile reports readiness rather than a phase.
 *
 * The headline is about **the rig and the board**; what is missing goes in the instruction. An
 * earlier version led with "No image", which read as a complaint from the status panel about
 * something the firmware panel was already saying, and told an operator nothing about whether
 * their board was even detected.
 */
function manualTile(state: RigState): Tile {
  if (state.busy) {
    return { headline: 'Flashing', instruction: 'Do not lift', tone: 'active' };
  }
  if (!state.targetPresent) {
    return { headline: 'No board', instruction: 'Seat a board on the fixture', tone: 'idle' };
  }
  if (!state.hasImage) {
    return { headline: 'Board detected', instruction: 'Choose firmware to flash', tone: 'active' };
  }
  return { headline: 'Ready', instruction: 'Read device, or Flash now', tone: 'ok' };
}

/** Auto-flash is the hands-free rhythm, and it is *this* that gets armed. */
function autoTile(state: RigState): Tile {
  switch (state.phase) {
    case 'disarmed':
      return { headline: 'Auto-flash', instruction: 'Arming…', tone: 'idle' };
    case 'probe-lost':
      return { headline: 'No probe', instruction: 'Check the ST-Link', tone: 'error' };
    case 'idle':
      return {
        headline: 'Armed',
        instruction: state.expect === 'flash' ? 'Seat a board' : 'Seat the flashed board again',
        tone: 'active',
      };
    case 'debouncing':
      return { headline: 'Contact…', instruction: 'Hold it steady', tone: 'active' };
    case 'flashing':
      return { headline: 'Flashing', instruction: 'Do not lift', tone: 'active' };
    case 'run-check':
      return { headline: 'Checking', instruction: 'Do not lift', tone: 'active' };
    case 'await-removal':
      return awaitRemovalTile(state.expect, state.lastCue);
  }
}

/**
 * The one state whose meaning depends on how it was reached.
 *
 * Waiting for removal after a flash, after a final pass, and after a failure are three different
 * instructions and three different tones, and the phase alone cannot tell them apart — which is
 * why the last cue is an input here rather than only a sound.
 */
function awaitRemovalTile(expect: Expect, lastCue: Cue): Tile {
  if (lastCue === 'fail') {
    return { headline: 'Failed', instruction: 'Remove and set aside', tone: 'error' };
  }
  if (lastCue === 'pass') {
    return { headline: 'Pass', instruction: 'Remove — board done', tone: 'ok' };
  }
  if (lastCue === 'flashed-cycle-it' || expect === 'run-check') {
    return { headline: 'Flashed', instruction: 'Power-cycle and re-seat', tone: 'ok' };
  }
  return { headline: 'Waiting', instruction: 'Clear the fixture', tone: 'idle' };
}

/** Whether Flash now can do anything right now, and why not when it cannot. */
export function flashNowState(state: RigState): { enabled: boolean; reason: string } {
  if (state.mode === 'auto') {
    return { enabled: false, reason: 'auto-flash is armed' };
  }
  if (!state.probeConnected) return { enabled: false, reason: 'no probe' };
  if (!state.hasImage) return { enabled: false, reason: 'no image selected' };
  if (!state.targetPresent) return { enabled: false, reason: 'nothing in the fixture' };
  if (state.busy) return { enabled: false, reason: 'busy' };
  return { enabled: true, reason: '' };
}

/** Whether Read device can do anything right now. Reading needs no image. */
export function readDeviceState(state: RigState): { enabled: boolean; reason: string } {
  if (!state.probeConnected) return { enabled: false, reason: 'no probe' };
  if (!state.targetPresent) return { enabled: false, reason: 'nothing in the fixture' };
  if (state.busy) return { enabled: false, reason: 'busy' };
  return { enabled: true, reason: '' };
}

export type SoundAction =
  | { kind: 'none' }
  /** Loop until superseded. The held busy level. */
  | { kind: 'loop' }
  /** Stop any loop, then play this one-shot. */
  | { kind: 'play'; name: 'success' | 'failure' | 'tick_big' | 'tick_small' };

/**
 * Cue to sound.
 *
 * `busy` is a *level*, not an event: it loops for the whole of a pass, so its absence is what an
 * operator notices when a contact is lost mid-write. Everything else stops that loop first.
 *
 * The two success cues are deliberately different sounds. An operator who cannot tell "flashed,
 * now cycle it" from "this board is done" never performs the second insertion, and the rig would
 * then flash the next board while they were still waiting for a tone.
 */
export function soundFor(cue: Cue): SoundAction {
  switch (cue) {
    case 'busy':
      return { kind: 'loop' };
    case 'flashed-cycle-it':
      return { kind: 'play', name: 'tick_big' };
    case 'pass':
      return { kind: 'play', name: 'success' };
    case 'fail':
      return { kind: 'play', name: 'failure' };
    case 'armed':
    case 'rearmed':
    case 'disarmed':
      return { kind: 'play', name: 'tick_small' };
    case 'none':
      return { kind: 'none' };
  }
}

/** How a detected layout should read, and whether it is a production arrangement. */
export function layoutSummary(layout: Layout): { label: string; detail: string; tone: Tone } {
  switch (layout) {
    case 'split':
      return {
        label: 'bootloader + application',
        detail: 'the production arrangement; RS485 field update can reach it',
        tone: 'ok',
      };
    case 'flat':
      return {
        label: 'single flat image',
        detail: 'a no_bootloader build — it runs, but RS485 field update cannot reach it',
        tone: 'warn',
      };
    case 'erased':
      return { label: 'erased', detail: 'nothing programmed', tone: 'idle' };
    case 'unrecognised':
      return {
        label: 'unrecognised',
        detail: 'programmed, but no valid vector table where one should be',
        tone: 'error',
      };
    case 'unknown':
      return { label: 'not read', detail: 'press Read device', tone: 'idle' };
  }
}

/** A shortened hash for a readout that must fit on one line. */
export function shortHash(sha: string): string {
  return sha ? sha.slice(0, 12) : '—';
}

/** The status bar's one-line summary of the rig. */
export function statusSummary(state: RigState): { value: string; tone: Tone } {
  if (!state.probeConnected) return { value: 'no probe', tone: 'error' };
  if (state.mode === 'manual') {
    return { value: state.busy ? 'manual · flashing' : 'manual', tone: 'ok' };
  }
  return {
    value: `auto · ${state.phase}`,
    tone: state.phase === 'flashing' || state.phase === 'run-check' ? 'active' : 'ok',
  };
}
