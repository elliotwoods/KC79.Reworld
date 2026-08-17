/**
 * The page's pure half.
 *
 * Everything that decides what the rig *says* is a function of plain values, tested without a
 * bus and without jsdom, so the component is left with nothing but subscriptions and markup.
 * Same discipline as the framework's own `web/src/vision/devices.ts`.
 *
 * # Enum values are read by name
 *
 * Never by discriminant. The Rust side declares `/rig/phase` and `/rig/cue` as enumerations with
 * names, and a page keyed on `3` would invert silently the moment someone reordered the variant
 * list in `schema.rs`.
 */

/** Tone vocabulary shared with the framework's pills and status items. */
export type Tone = 'idle' | 'active' | 'ok' | 'error' | 'offline';

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

export interface Tile {
  /** Big enough to read across a bench. */
  headline: string;
  /** What the operator should physically do next. Empty when there is nothing to do. */
  instruction: string;
  tone: Tone;
}

/**
 * What the rig is doing, and what the operator should do about it.
 *
 * The instruction is the load-bearing half. An operator who is not watching the screen still
 * glances at it when a tone surprises them, and "cycle it" versus "next board" is the one thing
 * they need at that moment — the two success tones are otherwise only distinguishable by ear.
 */
export function tileFor(phase: Phase, expect: Expect, lastCue: Cue): Tile {
  switch (phase) {
    case 'disarmed':
      return { headline: 'Disarmed', instruction: 'Arm to begin', tone: 'idle' };
    case 'probe-lost':
      return { headline: 'No probe', instruction: 'Check the ST-Link', tone: 'error' };
    case 'idle':
      return {
        headline: 'Ready',
        instruction: expect === 'flash' ? 'Seat a board' : 'Seat the flashed board again',
        tone: 'active',
      };
    case 'debouncing':
      return { headline: 'Contact…', instruction: 'Hold it steady', tone: 'active' };
    case 'flashing':
      return { headline: 'Flashing', instruction: 'Do not lift', tone: 'active' };
    case 'run-check':
      return { headline: 'Checking', instruction: 'Do not lift', tone: 'active' };
    case 'await-removal':
      return awaitRemovalTile(expect, lastCue);
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

/** What the browser should do with `SystemSounds` when a cue arrives. */
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
      return { kind: 'play', name: 'tick_small' };
    case 'disarmed':
      return { kind: 'play', name: 'tick_small' };
    case 'none':
      return { kind: 'none' };
  }
}

/** A shortened hash for a readout that must fit on one line. */
export function shortHash(sha: string): string {
  return sha ? sha.slice(0, 12) : '—';
}

/** The status bar's one-line summary of the rig. */
export function statusSummary(
  armed: boolean,
  phase: Phase,
  probePresent: boolean,
): { value: string; tone: Tone } {
  if (!probePresent) return { value: 'no probe', tone: 'error' };
  if (!armed) return { value: 'disarmed', tone: 'idle' };
  return { value: phase, tone: phase === 'flashing' || phase === 'run-check' ? 'active' : 'ok' };
}
