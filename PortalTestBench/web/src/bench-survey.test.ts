import { beforeEach, describe, expect, it } from 'vitest';
import { loadSurvey, loadSurveyForGeneration, resetSurveyStore, surveySnapshot } from './bench-survey';

const PROBE = {
  ports: [{ name: '/dev/cu.usbmodem5103', kind: 'usb', serial_number: 'PROBE123' }],
  probes: [{ identifier: '0483:374b:PROBE123', name: 'STLink V2-1', serial_number: 'PROBE123', kind: 'ST-LINK' }],
  swd_support: true,
  generation: 4,
};
const NOTHING = { ports: [], probes: [], swd_support: true, generation: 3 };

/** A fetch that resolves with `body` after `delayMs`, so ordering can be forced. */
function slowFetch(body: unknown, delayMs: number, ok = true, status = 200) {
  return () => new Promise<Response>((resolve) => {
    setTimeout(() => resolve({
      ok,
      status,
      json: async () => body,
    } as Response), delayMs);
  });
}

describe('the shared hardware survey', () => {
  beforeEach(resetSurveyStore);

  /**
   * The bug the ticket scheme exists for. A Rescan pressed while a refresh was in flight used to
   * receive the older request's promise, so the button appeared to work and changed nothing — and
   * on this bench the older request was the one taken before the fixture was plugged in.
   */
  it('lets the newest answer win, however slowly the older one lands', async () => {
    const first = loadSurvey(slowFetch(NOTHING, 30) as typeof fetch);
    const second = loadSurvey(slowFetch(PROBE, 1) as typeof fetch);
    await Promise.all([first, second]);
    expect(surveySnapshot().probes).toHaveLength(1);
    expect(surveySnapshot().probes[0].identifier).toBe('0483:374b:PROBE123');
  });

  it('keeps the last good document when a request fails, and says why', async () => {
    await loadSurvey(slowFetch(PROBE, 0) as typeof fetch);
    await loadSurvey((() => Promise.reject(new Error('host restarting'))) as unknown as typeof fetch);
    const after = surveySnapshot();
    expect(after.probes).toHaveLength(1);
    expect(after.error).toContain('host restarting');
    expect(after.loaded).toBe(true);
  });

  /** A 500 is a failed ask, not an empty bench. Emptying the list would state the wrong fact. */
  it('does not empty the list on a non-ok response', async () => {
    await loadSurvey(slowFetch(PROBE, 0) as typeof fetch);
    await loadSurvey(slowFetch(null, 0, false, 500) as typeof fetch);
    expect(surveySnapshot().probes).toHaveLength(1);
    expect(surveySnapshot().error).toContain('500');
  });

  it('fetches once per generation however many consumers watch it', async () => {
    let calls = 0;
    const answer = slowFetch(PROBE, 0);
    const counted = (() => {
      calls += 1;
      return answer();
    }) as typeof fetch;
    loadSurveyForGeneration(2, counted);
    loadSurveyForGeneration(2, counted);
    loadSurveyForGeneration(2, counted);
    expect(calls).toBe(1);
    loadSurveyForGeneration(3, counted);
    expect(calls).toBe(2);
    await Promise.resolve();
  });

  it('reports nothing loaded before the first answer', () => {
    expect(surveySnapshot().loaded).toBe(false);
    expect(surveySnapshot().probes).toEqual([]);
  });
});
