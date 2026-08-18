import { describe, expect, it } from 'vitest';
import { type BenchView, enumName, thresholdTone, verdictTile, whyDisabled } from './bench-model';

const ready: BenchView = {
  connected: true,
  transportObserved: 'vcp',
  modulePresent: true,
  firmwareKind: 'production',
  runBusy: false,
  runPlan: '',
  runPhase: 'idle',
  runOrigin: 'none',
  stepName: '',
  stepIndex: 0,
  stepCount: 0,
  cycle: 0,
  cycleCount: 0,
  lastVerdict: 'none',
  lastPlan: '',
  lastReason: '',
};

describe('verdictTile', () => {
  it('never returns a blank word or a blank detail', () => {
    const states: BenchView[] = [
      ready,
      { ...ready, connected: false },
      { ...ready, modulePresent: false },
      { ...ready, runBusy: true },
      { ...ready, lastVerdict: 'pass', lastPlan: 'routine-drive' },
      { ...ready, lastVerdict: 'fail', lastPlan: 'routine-drive' },
      { ...ready, lastVerdict: 'aborted' },
      { ...ready, lastVerdict: 'error' },
    ];
    for (const state of states) {
      const tile = verdictTile(state);
      expect(tile.word.length).toBeGreaterThan(0);
      expect(tile.detail.length).toBeGreaterThan(0);
    }
  });

  it('distinguishes no-link, no-module and ready rather than showing one idle box', () => {
    const words = [
      verdictTile({ ...ready, connected: false }).word,
      verdictTile({ ...ready, modulePresent: false }).word,
      verdictTile(ready).word,
    ];
    expect(new Set(words).size).toBe(3);
  });

  it('reports a dead link even when a stale passing verdict is still on the bus', () => {
    // A result from before the cable came out is not a result about now.
    const tile = verdictTile({ ...ready, connected: false, lastVerdict: 'pass', lastPlan: 'routine-drive' });
    expect(tile.word).toBe('NO LINK');
    expect(tile.tone).toBe('error');
  });

  it('names the firmware routine in flight, not just "running"', () => {
    const tile = verdictTile({
      ...ready,
      runBusy: true,
      runPlan: 'routine-drive',
      stepName: 'Home A',
      stepIndex: 3,
      stepCount: 9,
    });
    expect(tile.word).toBe('RUNNING');
    expect(tile.detail).toContain('Home A');
    expect(tile.detail).toContain('step 4 of 9');
  });

  it('says so when an agent is driving', () => {
    const tile = verdictTile({ ...ready, runBusy: true, runPlan: 'soak-8h', runOrigin: 'agent' });
    expect(tile.detail).toContain('agent');
  });

  it('shows the failing criterion rather than a generic failure', () => {
    const tile = verdictTile({
      ...ready,
      lastVerdict: 'fail',
      lastPlan: 'routine-drive',
      lastReason: 'backlash_usteps 1240 > 900',
    });
    expect(tile.detail).toBe('backlash_usteps 1240 > 900');
  });
});

describe('thresholdTone', () => {
  it('treats never-calibrated as alarming, not as zero', () => {
    const t = thresholdTone({ floor: 0, band: 0, applied: 0, calibratedAtS: -1 });
    expect(t.tone).toBe('error');
    expect(t.text).toContain('never calibrated');
  });

  it('accepts the measured production ring band', () => {
    // Final injection-moulded ring, uncovered: floor 240, shoulder 252, operating point 247.
    expect(thresholdTone({ floor: 240, band: 13, applied: 247, calibratedAtS: 12 }).tone).toBe('ok');
  });

  it('refuses the two-count band a physical cover produces', () => {
    expect(thresholdTone({ floor: 252, band: 2, applied: 252, calibratedAtS: 12 }).tone).toBe('error');
  });

  it('warns on a band narrow enough that a night of drift would close it', () => {
    expect(thresholdTone({ floor: 244, band: 6, applied: 247, calibratedAtS: 12 }).tone).toBe('warn');
  });
});

describe('whyDisabled', () => {
  it('gives a reason rather than just being false', () => {
    expect(whyDisabled({ ...ready, connected: false }, {}, false)).toBe('no link');
    expect(whyDisabled({ ...ready, modulePresent: false }, {}, false)).toContain('no module');
    expect(whyDisabled({ ...ready, runBusy: true }, {}, false)).toContain('run is in flight');
  });

  it('names both firmware kinds when a plan needs the other one', () => {
    const why = whyDisabled(ready, { needsFirmware: 'bench' }, false);
    expect(why).toContain('bench');
    expect(why).toContain('production');
  });

  it('allows an operation when everything it needs is present', () => {
    expect(whyDisabled(ready, {}, false)).toBeNull();
  });

  it('blocks starting destructive work from a stale page, but only destructive work', () => {
    expect(whyDisabled(ready, { destructive: true }, true)).toContain('lost contact');
    expect(whyDisabled(ready, { destructive: false }, true)).toBeNull();
  });
});

describe('enumName', () => {
  const variants = [
    [0, 'none'],
    [1, 'vcp'],
    [2, 'bench-ascii'],
  ] as const;

  it('reads by name', () => {
    expect(enumName(variants, 2)).toBe('bench-ascii');
  });

  it('degrades to "unknown" rather than throwing when the schema has moved on', () => {
    expect(enumName(variants, 99)).toBe('unknown');
  });
});
