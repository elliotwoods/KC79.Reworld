import { describe, expect, it } from 'vitest';
import {
  clampToUnitCircle,
  dialAngle,
  dialValue,
  diskPoint,
  diskPosition,
  formatUptime,
  gridCell,
  gridHit,
  heartbeatAlpha,
  previewIndex,
  slotOf,
  wallColumnX,
  wallHit,
  wallWidth,
} from './math';

describe('disk warp', () => {
  it('round-trips positions through the pow(r, 0.4) warp', () => {
    for (const [x, y] of [
      [0.5, 0],
      [0, -0.8],
      [0.3, 0.4],
      [-1, 0],
    ]) {
      const { dx, dy } = diskPoint(x, y);
      const back = diskPosition(dx, dy);
      expect(back.x).toBeCloseTo(x, 5);
      expect(back.y).toBeCloseTo(y, 5);
    }
  });

  it('inverts screen y (installation +y is up)', () => {
    expect(diskPoint(0, 1).dy).toBeLessThan(0);
    expect(diskPosition(0, -1).y).toBeCloseTo(1, 5);
  });

  it('warps small radii outward for finer centre control', () => {
    const { dx } = diskPoint(0.1, 0);
    expect(dx).toBeGreaterThan(0.1); // 0.1^0.4 ≈ 0.398
    expect(dx).toBeCloseTo(0.1 ** 0.4, 5);
  });

  it('clamps pointer input outside the disk to r = 1', () => {
    const p = diskPosition(2, 0);
    expect(Math.hypot(p.x, p.y)).toBeCloseTo(1, 5);
  });
});

describe('axis dial', () => {
  it('places 0 at the left, 0.25 up-ish per clockwise screen convention', () => {
    expect(dialAngle(0)).toBeCloseTo(-Math.PI, 5);
    expect(dialAngle(0.5)).toBeCloseTo(0, 5); // right
  });

  it('round-trips value -> angle -> value', () => {
    for (const value of [0, 0.1, 0.25, 0.5, 0.75, 0.999]) {
      const angle = dialAngle(value);
      expect(dialValue(Math.cos(angle), Math.sin(angle))).toBeCloseTo(value % 1, 4);
    }
  });
});

describe('portal grid', () => {
  it('puts portal 1 at the bottom when not flipped', () => {
    // 3 wide x 6 tall: portal 1 = index 0 -> gy = 5 (bottom row)
    expect(gridCell(0, 3, 6, false)).toEqual({ gx: 0, gy: 5 });
    expect(gridCell(0, 3, 6, true)).toEqual({ gx: 0, gy: 0 });
    expect(gridCell(17, 3, 6, false)).toEqual({ gx: 2, gy: 0 });
  });

  it('hit test inverts the cell mapping', () => {
    const [w, h, cx, cy] = [90, 180, 3, 6];
    for (let i = 0; i < 18; i++) {
      const { gx, gy } = gridCell(i, cx, cy, false);
      const px = (gx + 0.5) * (w / cx);
      const py = (gy + 0.5) * (h / cy);
      expect(gridHit(px, py, w, h, cx, cy, false, 18)).toBe(i);
    }
    expect(gridHit(-1, 0, w, h, cx, cy, false, 18)).toBe(-1);
  });
});

describe('the wall', () => {
  // Today's shape: 4 columns × (3×6), 40px cells, 6px separators.
  const [cols, cx, cy, cell, sep] = [4, 3, 6, 40, 6];

  it('lays columns edge to edge with one separator between', () => {
    expect(wallColumnX(0, cx, cell, sep)).toBe(0);
    expect(wallColumnX(1, cx, cell, sep)).toBe(126);
    expect(wallWidth(cols, cx, cell, sep)).toBe(4 * 120 + 3 * 6);
  });

  it('hits the right portal across separators, bottom-up', () => {
    // centre of column 2's bottom-left cell (portal 1)
    const x = wallColumnX(2, cx, cell, sep) + cell / 2;
    const y = (cy - 1) * cell + cell / 2;
    expect(wallHit(x, y, cols, cx, cy, false, cell, sep)).toEqual({ col: 2, index: 0 });
    // top-right cell of the last column = last portal
    expect(
      wallHit(wallColumnX(3, cx, cell, sep) + 2.5 * cell, 2, cols, cx, cy, false, cell, sep),
    ).toEqual({ col: 3, index: 17 });
    // flipped: portal 1 at the top
    expect(wallHit(x, cell / 2, cols, cx, cy, true, cell, sep)).toEqual({ col: 2, index: 0 });
  });

  it('answers null in a separator and outside the wall', () => {
    const inSeparator = wallColumnX(0, cx, cell, sep) + cx * cell + sep / 2;
    expect(wallHit(inSeparator, 10, cols, cx, cy, false, cell, sep)).toBeNull();
    expect(wallHit(-1, 10, cols, cx, cy, false, cell, sep)).toBeNull();
    expect(wallHit(10, cy * cell + 1, cols, cx, cy, false, cell, sep)).toBeNull();
    expect(
      wallHit(wallWidth(cols, cx, cell, sep) + 1, 10, cols, cx, cy, false, cell, sep),
    ).toBeNull();
  });

  it('maps a portal to the preview pixel Column::update_positions_from_image samples', () => {
    // 12×6 image; portal 1 (index 0) of column 0 samples the bottom-left pixel when not flipped
    expect(previewIndex(0, 0, cx, cy, false, 12, 6)).toBe(5 * 12);
    // portal 4 (index 3, second row) of column 1 samples x=3, y=4
    expect(previewIndex(1, 3, cx, cy, false, 12, 6)).toBe(4 * 12 + 3);
    // flipped: rows run top-down
    expect(previewIndex(0, 0, cx, cy, true, 12, 6)).toBe(0);
    // out of the image
    expect(previewIndex(5, 0, cx, cy, false, 12, 6)).toBeNull();
  });
});

describe('helpers', () => {
  it('slots accumulate across heterogeneous columns', () => {
    expect(slotOf([0, 18, 36], 1, 5)).toBe(22);
  });

  it('heartbeat fades over 300 ms', () => {
    expect(heartbeatAlpha(0)).toBe(1);
    expect(heartbeatAlpha(150)).toBeCloseTo(0.5, 5);
    expect(heartbeatAlpha(400)).toBe(0);
    expect(heartbeatAlpha(Number.NaN)).toBe(0);
  });

  it('clamps to the unit circle', () => {
    expect(clampToUnitCircle(3, 4)).toEqual({ x: 0.6, y: 0.8 });
    expect(clampToUnitCircle(0.3, 0.4)).toEqual({ x: 0.3, y: 0.4 });
  });

  it('formats uptime like the C++ app', () => {
    expect(formatUptime(90_061_000)).toBe('1d 1h 1m 1s');
    expect(formatUptime(61_000)).toBe('1m 1s');
  });
});
