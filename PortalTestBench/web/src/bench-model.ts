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

/** A badge tone. `Tone` also has `busy`, which no card in the action strip can be. */
export type BoardTone = Extract<Tone, 'ok' | 'warn' | 'error' | 'idle'>;

/**
 * One card in the action strip: an entered value held against what the board holds.
 *
 * Three facts, three places, and none repeats another: the field is what the operator entered,
 * `detail` is what the board holds, and `word` is the relation between them. The old serial card
 * said "EXISTING" in the badge and "matches on-board serial" underneath — one fact twice — so the
 * rule here is that `detail` is a function of the board alone and never restates `word`.
 */
export interface BoardValue {
  /** The badge: on board, changed, fresh, defaults, review, pending, no board, DB offline. */
  word: string;
  /** What the board holds. */
  detail: string;
  /** What this state means and what to do about it — the hover text on the badge. */
  hint: string;
  tone: BoardTone;
  /** The entered value differs from the board's, so the next flash pass will write it. */
  changed: boolean;
}

export interface SerialView {
  dbOk: boolean;
  boardPresent: boolean;
  /** `/provision/identity_state`: blank | corrupt | foreign-uid | existing-on-board | unknown. */
  identity: string;
  /** `/provision/on_board_serial`; 0 when the board carries none. */
  boardSerial: number;
  entered: number;
  pending: boolean;
}

export interface SettingsView {
  boardPresent: boolean;
  identity: string;
  pending: boolean;
  /** `/provision/settings/source`: `flash` when a journal was read, `defaults` otherwise. */
  source: string;
  boardCurrentMa: number;
  boardRecovery: boolean;
  currentMa: number;
  recovery: boolean;
}

/** "150 mA · recovery on" — the one spelling of a settings pair, shared by the card and the panel. */
export function settingsSummary(ma: number, recovery: boolean): string {
  return `${ma} mA · recovery ${recovery ? 'on' : 'off'}`;
}

/** `unknown` is the identity of a board nobody has read yet: `flash.rs` always sets one of the other four. */
const boardRead = (v: { boardPresent: boolean; identity: string }): boolean => v.boardPresent && v.identity !== 'unknown';

/**
 * The SERIAL NUMBER card.
 *
 * Blockers first, then the comparison. `review` survives from the old vocabulary for the two
 * identity anomalies only: the worker refuses to flash a corrupt or foreign-UID board without an
 * explicit override, so neither "on board" nor "fresh" would be true — and a foreign-UID board
 * whose record serial happened to equal the entered one used to read as a green "existing".
 */
export function serialState(v: SerialView): BoardValue {
  const changed = boardRead(v) && v.boardSerial > 0 && v.entered !== v.boardSerial;
  const detail = !v.boardPresent
    ? 'waiting for a board'
    : v.identity === 'unknown'
      ? 'board: not read yet'
      : v.identity === 'corrupt'
        ? 'board: identity corrupt'
        : v.identity === 'foreign-uid'
          ? `board: ${v.boardSerial} · foreign UID`
          : v.identity === 'blank'
            ? 'board: no serial'
            : `board: ${v.boardSerial}`;
  if (!v.dbOk) return { word: 'DB offline', detail, hint: 'The provisioning database is unavailable, so nothing can be flashed or provisioned.', tone: 'error', changed };
  if (v.pending) return { word: 'pending', detail: 'board: replug to verify', hint: 'Provisioned. Unplug and replug the board so its UID, serial and firmware can be verified without another flash.', tone: 'warn', changed: false };
  if (!v.boardPresent) return { word: 'no board', detail, hint: 'No MCU is answering the probe.', tone: 'idle', changed: false };
  if (!boardRead(v)) return { word: 'no board', detail, hint: 'A board is attached but its identity has not been read yet.', tone: 'idle', changed: false };
  if (v.identity === 'corrupt') return { word: 'review', detail, hint: 'The identity page is corrupt. Flashing needs an explicit choice in the Flash tab.', tone: 'warn', changed: false };
  if (v.identity === 'foreign-uid') return { word: 'review', detail, hint: 'The identity page was written for a different MCU. Flashing needs an explicit choice in the Flash tab.', tone: 'warn', changed: false };
  if (v.identity === 'blank') return { word: 'fresh', detail, hint: 'The board has no serial yet; the next flash writes this one.', tone: 'idle', changed: false };
  if (changed) return { word: 'changed', detail, hint: 'Differs from the serial on the board. Before flashing, choose in the Flash tab: keep the on-board serial, or use the entered one.', tone: 'warn', changed };
  return { word: 'on board', detail, hint: 'The entered serial is the one on the board.', tone: 'ok', changed: false };
}

/**
 * The MODULE SETTINGS card: operating current and full-current home recovery.
 *
 * These reach the board in the same pass as the serial, so "changed" means the same thing on
 * both cards: the next flash writes it. "pending" is shared for the same reason — until the
 * replug the on-board values are stale or unverified. "defaults" is distinct from "on board"
 * because a board with no journal is running firmware defaults, and the pass writes a journal
 * whenever that is so, even when the numbers agree.
 */
export function settingsState(v: SettingsView): BoardValue {
  const read = boardRead(v);
  const changed = read && (v.currentMa !== v.boardCurrentMa || v.recovery !== v.boardRecovery);
  const detail = !v.boardPresent
    ? 'waiting for a board'
    : !read
      ? 'board: not read yet'
      : v.source === 'defaults'
        ? 'board: no settings stored'
        : `board: ${settingsSummary(v.boardCurrentMa, v.boardRecovery)}`;
  if (v.pending) return { word: 'pending', detail: 'board: replug to verify', hint: 'Written with the serial. Unplug and replug the board to verify what it holds.', tone: 'warn', changed: false };
  if (!v.boardPresent) return { word: 'no board', detail, hint: 'No MCU is answering the probe.', tone: 'idle', changed: false };
  if (!read) return { word: 'no board', detail, hint: 'A board is attached but its settings have not been read yet.', tone: 'idle', changed: false };
  if (changed) return { word: 'changed', detail, hint: 'Differs from what the board holds. The next flash — or Write to board in the Flash tab — stores these values.', tone: 'warn', changed };
  if (v.source === 'defaults') return { word: 'defaults', detail, hint: 'The board has no settings journal and runs firmware defaults; the next flash writes one.', tone: 'idle', changed: false };
  return { word: 'on board', detail, hint: 'The entered settings are what the board holds.', tone: 'ok', changed: false };
}

/** One flashable image, as `/api/bench/firmware` lists it. */
export interface FirmwareItem {
  id: string;
  region: 'bootloader' | 'application';
  /** `built` from this tree, or a committed `reference` image. */
  origin: string;
  bytes: number;
  /** File mtime, seconds since the epoch. */
  modified?: number | null;
  /** `optical` / `mechanical`, application banks only. */
  variant?: string | null;
  /** `PCB v6` / `PCB v4`, application banks only. */
  hardware?: string | null;
  /** `Portal v2026-08-25_17.34 8799276+` or `Bootloader v5`, scraped from the file. */
  banner?: string | null;
}

/**
 * Title and detail for one row of the firmware picker.
 *
 * The bank header names the product (PortalFW Application, PortalBootloader), so a row names the
 * *build*: the PCB it targets, then its version, when it was built and how big it is. The
 * banner's product prefix is dropped because the header already said it. A reference image has
 * no build time worth showing -- its mtime is when the checkout happened -- so it is named by
 * its file instead.
 */
export function firmwareRow(item: FirmwareItem): { title: string; detail: string } {
  const title = item.variant && item.hardware
    ? `${item.variant[0].toUpperCase()}${item.variant.slice(1)} · ${item.hardware}`
    : item.origin === 'reference'
      ? 'Reference image'
      : 'Built from source';
  const version = item.banner?.replace(/^(Portal|Bootloader) /, '') ?? null;
  const when = item.origin === 'reference'
    ? item.id.replace(/^reference:/, '')
    : item.modified
      ? `built ${new Date(item.modified * 1000).toLocaleString([], { dateStyle: 'medium', timeStyle: 'short' })}`
      : null;
  const size = `${(item.bytes / 1024).toFixed(1)} kB`;
  return { title, detail: [version, when, size].filter(Boolean).join(' · ') };
}
