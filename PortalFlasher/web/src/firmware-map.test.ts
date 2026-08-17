import { describe, expect, it } from 'vitest';

import {
  COLUMNS,
  type Occupancy,
  buildMap,
  comparisonSummary,
  fillOf,
  formatBytes,
  regionSectors,
  rowAddress,
  sectorTitle,
  usedSectors,
} from './firmware-map';

const SECTORS = 64;
const OPTS = { sectorBytes: 2048, bootloaderSectors: 12, flashBase: 0x0800_0000 };

/** A device programmed for its first `n` sectors and erased after. */
function programmed(n: number): Occupancy[] {
  return Array.from({ length: SECTORS }, (_, i) => (i < n ? 255 : 0));
}

describe('fillOf', () => {
  it('separates erased, partly programmed and full', () => {
    expect(fillOf(0)).toBe('erased');
    expect(fillOf(255)).toBe('programmed');
    expect(fillOf(128)).toBe('partial');
  });

  it('shows a single programmed byte rather than rounding it away', () => {
    expect(fillOf(1)).toBe('partial');
  });
});

describe('buildMap', () => {
  it('draws one cell per real flash sector', () => {
    const model = buildMap(programmed(SECTORS), null, OPTS);
    expect(model.sectors).toHaveLength(64);
    expect(model.sectorBytes).toBe(2048);
    // 64 sectors of 2 kB is the whole 128 kB part, which is the point of using sectors at all.
    expect(model.sectors.length * model.sectorBytes).toBe(128 * 1024);
  });

  it('splits the banks where the hardware does', () => {
    const model = buildMap(programmed(SECTORS), null, OPTS);
    expect(regionSectors(model, 'bootloader')).toBe(12);
    expect(regionSectors(model, 'application')).toBe(52);
    // 24 kB in 2 kB sectors is exactly 12 -- which is what makes bootloader-only and
    // application-only flashing sector-aligned rather than a partial-erase problem.
    expect(12 * 2048).toBe(24 * 1024);
  });

  it('gives every sector its real address', () => {
    const model = buildMap(programmed(1), null, OPTS);
    expect(model.sectors[0].address).toBe(0x0800_0000);
    expect(model.sectors[12].address).toBe(0x0800_6000);
    expect(model.sectors[63].address).toBe(0x0801_F800);
  });

  it('draws nothing before a read', () => {
    const model = buildMap(null, null, OPTS);
    expect(model.sectors).toHaveLength(0);
    expect(comparisonSummary(model)).toBeNull();
  });

  it('says nothing about a comparison when nothing is selected', () => {
    // Saying "no image selected" here read as a complaint about the flash contents rather than
    // about the firmware picker, which is a different panel's business.
    const model = buildMap(programmed(40), null, OPTS);
    expect(model.diffCount).toBeNull();
    expect(comparisonSummary(model)).toBeNull();
    expect(model.sectors.every((s) => s.differs === false)).toBe(true);
  });

  it('reports a match as a match', () => {
    const same = programmed(40);
    const model = buildMap(same, [...same], OPTS);
    expect(model.diffCount).toBe(0);
    expect(comparisonSummary(model)).toEqual({
      label: 'matches the selected image',
      tone: 'ok',
    });
  });

  it('counts differing sectors, and says how many', () => {
    const model = buildMap(programmed(20), programmed(24), OPTS);
    expect(model.diffCount).toBe(4);
    expect(comparisonSummary(model)?.label).toBe('4 sectors differ from the selected image');
    expect(comparisonSummary(model)?.tone).toBe('warn');
  });

  it('gets the singular right', () => {
    const model = buildMap(programmed(20), programmed(21), OPTS);
    expect(model.diffCount).toBe(1);
    expect(comparisonSummary(model)?.label).toBe('1 sector differs from the selected image');
  });

  it('ignores differences too small to act on', () => {
    // The occupancy is a fraction of a 2 kB sector; 3/2048 versus 4/2048 is not actionable.
    const a = [2, ...Array(SECTORS - 1).fill(0)];
    const b = [3, ...Array(SECTORS - 1).fill(0)];
    expect(buildMap(a, b, OPTS).diffCount).toBe(0);
    // Erased versus programmed always counts.
    expect(buildMap([0, ...Array(SECTORS - 1).fill(0)], b, OPTS).diffCount).toBe(1);
  });

  it('does not silently shorten the grid when the selected image is shorter', () => {
    const model = buildMap(programmed(SECTORS), programmed(10).slice(0, 32), OPTS);
    expect(model.sectors).toHaveLength(64);
    expect(model.missing).toBe(32);
  });
});

describe('usedSectors', () => {
  it('counts what is programmed in each bank', () => {
    // A flat image: everything programmed, so both banks report used.
    const flat = buildMap(programmed(50), null, OPTS);
    expect(usedSectors(flat, 'bootloader')).toBe(12);
    expect(usedSectors(flat, 'application')).toBe(38);

    // An erased part.
    const blank = buildMap(programmed(0), null, OPTS);
    expect(usedSectors(blank, 'bootloader')).toBe(0);
    expect(usedSectors(blank, 'application')).toBe(0);
  });
});

describe('rowAddress', () => {
  it('labels each row with the address it starts at', () => {
    const model = buildMap(programmed(SECTORS), null, OPTS);
    expect(rowAddress(model, 0)).toBe('0x08000000');
    // Eight sectors of 2 kB is 16 kB a row, so every row boundary is a round address.
    expect(rowAddress(model, 1)).toBe('0x08004000');
    expect(rowAddress(model, 7)).toBe('0x0801C000');
    expect(COLUMNS * 2048).toBe(16 * 1024);
  });
});

describe('sectorTitle', () => {
  it('says everything about one sector in a sentence', () => {
    const model = buildMap(programmed(20), programmed(24), OPTS);
    const title = sectorTitle(model.sectors[21], model.sectorBytes);
    expect(title).toContain('0x0800A800');
    expect(title).toContain('sector 21');
    expect(title).toContain('2 kB');
    expect(title).toContain('application');
    expect(title).toContain('differs');
  });

  it('does not claim a difference when there is nothing to compare', () => {
    const model = buildMap(programmed(20), null, OPTS);
    expect(sectorTitle(model.sectors[0], model.sectorBytes)).not.toContain('differs');
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
