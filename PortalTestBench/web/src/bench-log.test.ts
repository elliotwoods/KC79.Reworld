import { describe, expect, it } from 'vitest';
import { type LogLine, splitLogLines } from './bench-log';

const line = (seq: number, source: string): LogLine => ({
  seq,
  source,
  at_ms: seq * 10,
  level: 0,
  message: `line ${seq}`,
});

describe('session log split view', () => {
  it('puts only VCOM firmware lines in the serial pane and preserves order', () => {
    const split = splitLogLines([
      line(1, 'bench'),
      line(2, 'serial'),
      line(3, 'rs485'),
      line(4, 'serial'),
    ]);

    expect(split.serial.map(({ seq }) => seq)).toEqual([2, 4]);
    expect(split.rest.map(({ seq }) => seq)).toEqual([1, 3]);
  });
});
