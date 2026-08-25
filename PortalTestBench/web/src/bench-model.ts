/**
 * Pure view logic for the bench page.
 *
 * Everything here is a plain function of plain data: no React, no bus, no DOM. That is what
 * makes it testable, and the things it decides — what the verdict band says, whether the
 * threshold is trustworthy, whether an operation is allowed — are exactly the things that must
 * not be wrong. The component file below it should read as layout.
 */

export type Tone = 'ok' | 'warn' | 'error' | 'busy' | 'idle';

export type Cue = 'none' | 'connected' | 'lost' | 'run-start' | 'pass' | 'fail' | 'abort' | 'attention';

export type SoundAction =
  | { kind: 'none' }
  | { kind: 'loop' }
  | { kind: 'play'; name: 'success' | 'failure' | 'tick_big' | 'tick_small' };

/** Map the worker's one-shot cue stream onto the framework's shipped system sounds. */
export function soundFor(cue: Cue): SoundAction {
  switch (cue) {
    case 'run-start':
      return { kind: 'loop' };
    case 'pass':
      return { kind: 'play', name: 'success' };
    case 'fail':
    case 'lost':
    case 'attention':
      return { kind: 'play', name: 'failure' };
    case 'abort':
      return { kind: 'play', name: 'tick_big' };
    case 'connected':
      return { kind: 'play', name: 'tick_small' };
    case 'none':
      return { kind: 'none' };
  }
}

/** What the verdict band shows. Never blank, never ambiguous. */
export interface Tile {
  /** The single word (or short phrase) that is the current answer. */
  word: string;
  /** The sentence under it. Always present: a tile with no explanation is a grey box. */
  detail: string;
  tone: Tone;
}

export interface BenchView {
  connected: boolean;
  transportObserved: string;
  modulePresent: boolean;
  firmwareKind: string;
  runBusy: boolean;
  runPlan: string;
  runPhase: string;
  runOrigin: string;
  stepName: string;
  stepIndex: number;
  stepCount: number;
  cycle: number;
  cycleCount: number;
  lastVerdict: string;
  lastPlan: string;
  lastReason: string;
}

/**
 * The verdict band.
 *
 * The ordering matters more than the wording: "no link" outranks "no module", which outranks
 * any stale verdict, because a result from before the cable came out is not a result about
 * now. Three different not-running states get three different tiles — collapsing them into one
 * "idle" is how an operator ends up waiting on a bench that was never going to do anything.
 */
export function verdictTile(v: BenchView): Tile {
  if (!v.connected) {
    return {
      word: 'NO LINK',
      detail:
        v.transportObserved === 'none'
          ? 'Pick a transport and connect.'
          : `The ${v.transportObserved} link is down.`,
      tone: 'error',
    };
  }
  if (!v.modulePresent) {
    return { word: 'NO MODULE', detail: 'The link is open but nothing has answered on it.', tone: 'warn' };
  }
  if (v.runBusy) {
    return { word: 'RUNNING', detail: runningDetail(v), tone: 'busy' };
  }
  switch (v.lastVerdict) {
    case 'pass':
      return { word: 'PASS', detail: `${v.lastPlan} met every criterion.`, tone: 'ok' };
    case 'fail':
      return { word: 'FAIL', detail: v.lastReason || `${v.lastPlan} did not meet its criteria.`, tone: 'error' };
    case 'aborted':
      return { word: 'ABORTED', detail: v.lastReason || `${v.lastPlan} was stopped before it could decide.`, tone: 'warn' };
    case 'error':
      return { word: 'ERROR', detail: v.lastReason || `${v.lastPlan} could not be carried out.`, tone: 'error' };
    default:
      return {
        word: 'READY',
        detail: `${describeFirmware(v.firmwareKind)} module connected. No test has run yet.`,
        tone: 'idle',
      };
  }
}

/**
 * "Home A — step 4 of 9, cycle 12 of 400".
 *
 * Naming the firmware routine in flight is the point: the routines ACK immediately and then
 * run for seconds to minutes, so without this the page shows a spinner for a minute and looks
 * hung. Who started the run is included when it was not the person reading the screen.
 */
export function runningDetail(v: BenchView): string {
  const parts: string[] = [];
  if (v.stepName) parts.push(v.stepName);
  if (v.stepCount > 0) parts.push(`step ${v.stepIndex + 1} of ${v.stepCount}`);
  if (v.cycleCount > 1) parts.push(`cycle ${v.cycle} of ${v.cycleCount}`);
  if (v.runPhase && v.runPhase !== 'body') parts.push(v.runPhase);
  if (v.runOrigin === 'agent' || v.runOrigin === 'cli') parts.push(`started by ${v.runOrigin}`);
  const tail = parts.length > 0 ? ` — ${parts.join(', ')}` : '';
  return `${v.runPlan || 'a plan'}${tail}`;
}

function describeFirmware(kind: string): string {
  switch (kind) {
    case 'production':
      return 'A production-firmware';
    case 'bench':
      return 'A bench-firmware';
    case 'bootloader-only':
      return 'A bootloader-only';
    default:
      return 'An unidentified';
  }
}

/** How the optical threshold triple should read. */
export interface ThresholdView {
  floor: number;
  band: number;
  applied: number;
  /** Seconds since this session started, or -1 for "never". */
  calibratedAtS: number;
}

/**
 * The threshold summary that is on screen at all times.
 *
 * "Never calibrated" is a distinct, alarming state rather than a row of zeroes. The background
 * this used to be derived from is unmeasurable on the production ring, so the operating point
 * can only come from a per-run census of the flag itself — and a home whose threshold nobody
 * recorded is not evidence. Keeping this visible is what stops a fixed constant creeping back.
 */
export function thresholdTone(t: ThresholdView): { tone: Tone; text: string } {
  if (t.calibratedAtS < 0) {
    return { tone: 'error', text: 'never calibrated this session' };
  }
  // Measured: the unpainted production ring holds 9–11 counts overnight in an unlit room, but
  // collapses to 2 under a physical cover. Below about 4 there is no room for a single count
  // of thermal drift.
  if (t.band < 4) {
    return { tone: 'error', text: `band only ${t.band} counts — too narrow to home on` };
  }
  if (t.band < 8) {
    return { tone: 'warn', text: `band ${t.band} counts — narrow; check the cover and ambient light` };
  }
  return { tone: 'ok', text: `T=${t.applied} in a ${t.band}-count band from ${t.floor}` };
}

/** Why an operation cannot be started right now, or `null` if it can. */
export function whyDisabled(
  v: BenchView,
  op: { needsLink?: boolean; needsModule?: boolean; needsFirmware?: string; destructive?: boolean },
  uiStale: boolean,
): string | null {
  if (v.runBusy) return 'a run is in flight';
  if (op.needsLink !== false && !v.connected) return 'no link';
  if (op.needsModule !== false && !v.modulePresent) return 'no module is answering';
  if (op.needsFirmware && v.firmwareKind !== op.needsFirmware) {
    return `needs ${op.needsFirmware} firmware, this module is running ${v.firmwareKind}`;
  }
  // The dead-man is deliberately narrow here: a stale page blocks *starting* destructive work
  // and nothing else. It never cancels a run in flight, because closing a browser tab must not
  // abort an eight-hour soak. This inverts PortalFlasher's rule on purpose; see AGENTS.md.
  if (op.destructive && uiStale) return 'this page has lost contact with the bench';
  return null;
}

/** Look an enum value up by name. Never key a page on a discriminant. */
export function enumName(variants: ReadonlyArray<readonly [number, string]>, value: number): string {
  return variants.find(([discriminant]) => discriminant === value)?.[1] ?? 'unknown';
}

/** One communication lane, as the link controls see it. */
export interface LinkView {
  /** `serial` or `rs485` — the route commands are currently sent over. */
  route: string;
  connected: boolean;
  /** The chosen protocol or transport, by name. `none` when nothing has been picked. */
  desired: string;
  /** The port or address. Empty when nothing has been chosen. */
  endpoint: string;
  /** `/{lane}/detail` — the last thing the link said about itself, failure included. */
  detail: string;
}

/**
 * Transports that address themselves.
 *
 * `sim` is `SimBus` in this process and `none` is not a transport at all; neither has an
 * endpoint to type, and demanding one would block Connect on a field that should not exist.
 */
const ENDPOINTLESS = new Set(['none', 'sim']);

/**
 * Why **Connect** cannot be pressed, or `null` if it can.
 *
 * The reason this is a function and not an inline ternary: pressing Connect with no transport
 * chosen used to be *allowed*, and the worker answered by writing "pick a serial transport
 * first" into the session log. A button that is enabled, does nothing visible, and explains
 * itself somewhere the operator is not looking is worse than a disabled one — the tooltip on a
 * greyed button is on the pointer already.
 */
export function connectBlocker(v: LinkView): string | null {
  if (v.connected) return 'already connected';
  if (v.desired === 'none') return `choose a ${v.route === 'rs485' ? 'transport' : 'protocol'} first`;
  if (!ENDPOINTLESS.has(v.desired) && !v.endpoint) {
    return v.route === 'rs485' ? 'choose an endpoint' : 'choose a port';
  }
  return null;
}

/**
 * The sentence under the link controls when the selected route is down, or `null` when it is up.
 *
 * Every actionable control in the Test tab is gated on this one route being connected, so the
 * tab's resting state is a wall of greyed buttons. Saying why, once, at the top, is the
 * difference between an instrument that is waiting and one that looks broken. A previous
 * failure — which only reaches the page now that `Rs485Link::open` stopped reporting success
 * unconditionally — outranks the generic advice: it is the more specific answer.
 */
export function linkBlocker(v: LinkView): string | null {
  if (v.connected) return null;
  const route = v.route === 'rs485' ? 'RS485' : 'serial';
  const lead = `Nothing below can run until the ${route} link is open.`;
  if (v.detail) return `${lead} Last attempt: ${v.detail}`;
  const next = connectBlocker(v);
  return next ? `${lead} Pick a transport and connect.` : `${lead} Press Connect.`;
}
