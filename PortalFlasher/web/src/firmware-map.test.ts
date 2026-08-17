import { describe, expect, it } from 'vitest';

import {
  buildMap,
  comparisonSummary,
  diffBuckets,
  fillOf,
  formatBytes,
  ticks,
} from './firmware-map';

const BUCKETS = 256;

/** A device that is programmed for the first `fraction` of flash and erased after. */
function programmedUpTo(fraction: number): number[] {
  const cut = Math.round(BUCKETS * fraction);
  return Array.from({ length: BUCKETS }, (_, i) => (i < cut ? 255 : 0));
}

describe('fillOf', () => {
  it('separates erased, partly programmed and full', () => {
    expect(fillOf(0)).toBe('erased');
    expect(fillOf(255)).toBe('programmed');
    expect(fillOf(128)).toBe('partial');
  });

  it('shows a single programmed byte rather than rounding it away', () => {
    // One byte in a 512-byte bucket is 0, but the Rust side reports at least 1 for anything
    // non-erased. An operator wants to see that, not have it disappear.
    expect(fillOf(1)).toBe('partial');
  });
});

describe('diffBuckets', () => {
  it('flags where the lanes disagree', () => {
    const device = programmedUpTo(0.5);
    const selected = programmedUpTo(0.75);
    const diff = diffBuckets(device, selected);
    expect(diff).toHaveLength(BUCKETS);
    expect(diff.slice(0, 128).some(Boolean)).toBe(false);
    expect(diff.slice(128, 192).every(Boolean)).toBe(true);
    expect(diff.slice(192).some(Boolean)).toBe(false);
  });

  it('ignores differences too small to act on', () => {
    // The buckets are lossy. 3/512 versus 4/512 programmed is not a difference an operator can
    // do anything about, and flagging it would make every map look wrong.
    expect(diffBuckets([2], [3])).toEqual([false]);
    // But erased-versus-programmed always counts.
    expect(diffBuckets([0], [3])).toEqual([true]);
  });

  it('tolerates lanes of different lengths', () => {
    expect(diffBuckets([255, 255], [255])).toHaveLength(1);
  });
});

describe('buildMap', () => {
  it('draws one lane when nothing is selected, and does not imply agreement', () => {
    const model = buildMap(programmedUpTo(0.5), null, 0.1875);
    expect(model.lanes).toHaveLength(1);
    expect(model.diff).toBeNull();
    expect(comparisonSummary(model).label).toMatch(/no image selected/);
  });

  it('draws nothing before a read', () => {
    const model = buildMap(null, null, 0.1875);
    expect(model.lanes).toHaveLength(0);
    expect(comparisonSummary(model).label).toMatch(/not read/);
  });

  it('reports a match as a match', () => {
    const same = programmedUpTo(0.5);
    const model = buildMap(same, [...same], 0.1875);
    expect(model.diffCount).toBe(0);
    expect(comparisonSummary(model)).toEqual({
      label: 'matches the selected image',
      tone: 'ok',
    });
  });

  it('quantifies a mismatch', () => {
    const model = buildMap(programmedUpTo(0.25), programmedUpTo(0.75), 0.1875);
    expect(model.diffCount).toBeGreaterThan(0);
    const summary = comparisonSummary(model);
    expect(summary.tone).toBe('warn');
    expect(summary.label).toMatch(/differs across ~50%/);
  });

  it('never reports 0% for a real difference', () => {
    // One bucket out of 256 rounds to 0%, which would read as "identical" next to a warn tone.
    const device = programmedUpTo(0);
    const selected = [...device];
    selected[0] = 255;
    expect(comparisonSummary(buildMap(device, selected, 0.1875)).label).toMatch(/~1%/);
  });
});

describe('ticks', () => {
  it('marks the bank boundary and the ends', () => {
    const labels = ticks().map((t) => t.label);
    expect(labels).toEqual(['0', '24k', '64k', '128k']);
    // 24 kB of 128 kB is where the bootloader bank ends -- the one boundary that matters.
    expect(ticks()[1].at).toBeCloseTo(0.1875, 5);
    expect(ticks()[3].at).toBe(1);
  });
});

describe('formatBytes', () => {
  it('reads as a person would say it', () => {
    expect(formatBytes(0)).toBe('0');
    expect(formatBytes(512)).toBe('512 B');
    expect(formatBytes(22_708)).toBe('22.2 kB');
    expect(formatBytes(131_072)).toBe('128.0 kB');
  });
});
