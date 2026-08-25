/**
 * What is plugged into this machine, as one document the whole page shares.
 *
 * `GET /api/bench/ports`, fetched for the whole page rather than per component.
 *
 * Two pickers read it — the Flash tab's ST-Link list and the Test tab's endpoint list — and they
 * must agree, because pairing a probe to its VCOM port is done by matching the USB serial number
 * across the two halves of the *same* survey. Two independent fetches could disagree by a replug,
 * and the store also spares an IOKit enumeration per mount.
 *
 * Module scope rather than context: there is one bench, one machine and one answer.
 *
 * # Why it re-fetches
 *
 * It used to fetch once, on the first mount, and never again. A bench started before its fixture
 * was plugged in therefore showed "No ST-Link found. Connect the fixture probe and rescan." beside
 * a badge reading "connected", under a band reading "MCU connected", while flashing worked — the
 * badge is a live bus parameter and the list was a stale fetch. The worker now publishes
 * `/setup/ports_generation`, bumped only when the set of attached devices actually moves or when
 * somebody rescans, and the page re-reads on that. It is a notification, not a poll.
 */

export interface ProbeChoice { identifier: string; name?: string; serial_number?: string; kind: string }
export interface PortChoice { name: string; kind: string; product?: string; serial_number?: string }

export interface PortSurvey {
  ports: PortChoice[];
  probes: ProbeChoice[];
  swd_support: boolean;
  /** Mirrors `/setup/ports_generation`, so a caller can tell which bump this answer includes. */
  generation?: number;
}

export interface SurveyState extends PortSurvey {
  /** False until a request has succeeded. An empty list before that is not evidence of anything. */
  loaded: boolean;
  loading: boolean;
  /** Non-empty when the last attempt failed. The lists below are then the last good answer. */
  error: string;
}

const EMPTY: PortSurvey = { ports: [], probes: [], swd_support: false };

let value: PortSurvey = EMPTY;
let loaded = false;
let loading = false;
let error = '';
/**
 * Tickets, not a shared promise.
 *
 * The store used to return the in-flight promise to every caller, so a Rescan pressed while a
 * refresh was running resolved against a request issued *before* the operator plugged the fixture
 * in — the button appeared to work and changed nothing. A monotonic ticket lets every call issue
 * its own request and lets only the newest answer win, so a slow first response can never
 * overwrite a fast second one.
 */
let issued = 0;
let applied = 0;

const listeners = new Set<() => void>();

function announce() {
  listeners.forEach((listener) => listener());
}

export function subscribeSurvey(listener: () => void): () => void {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

export function surveySnapshot(): SurveyState {
  return { ...value, loaded, loading, error };
}

export async function loadSurvey(fetchImpl: typeof fetch = fetch): Promise<void> {
  const ticket = ++issued;
  loading = true;
  announce();
  try {
    const response = await fetchImpl('/api/bench/ports', { cache: 'no-store' });
    if (!response.ok) throw new Error(`the bench answered ${response.status}`);
    const body = await response.json() as PortSurvey;
    // An older request landing late must not undo a newer one.
    if (ticket > applied) {
      applied = ticket;
      value = body;
      loaded = true;
      error = '';
    }
  } catch (cause) {
    // Keep the last good document and say what went wrong. "The host is restarting" and "nothing
    // is plugged in" are different claims, and rendering them identically is how an empty list
    // came to be read as fact.
    if (ticket > applied) error = cause instanceof Error ? cause.message : String(cause);
  } finally {
    if (ticket === issued) loading = false;
    announce();
  }
}

/**
 * Fetch once per generation, across every mount.
 *
 * All three consumers of the survey watch the same parameter; without this they would each fire a
 * request on the same bump.
 */
let fetchedGeneration = -1;

export function loadSurveyForGeneration(generation: number, fetchImpl: typeof fetch = fetch): void {
  if (fetchedGeneration === generation) return;
  fetchedGeneration = generation;
  void loadSurvey(fetchImpl);
}

/** Test seam: forget everything the store has learned. */
export function resetSurveyStore(): void {
  value = EMPTY;
  loaded = false;
  loading = false;
  error = '';
  issued = 0;
  applied = 0;
  fetchedGeneration = -1;
  listeners.clear();
}
