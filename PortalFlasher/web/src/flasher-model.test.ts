import { describe, expect, it } from 'vitest';

import {
  type Cue,
  type Phase,
  shortHash,
  soundFor,
  statusSummary,
  tileFor,
} from './flasher-model';

describe('tileFor', () => {
  it('tells the operator what to physically do in every phase', () => {
    const phases: Phase[] = [
      'disarmed',
      'idle',
      'debouncing',
      'flashing',
      'run-check',
      'await-removal',
      'probe-lost',
    ];
    for (const phase of phases) {
      const tile = tileFor(phase, 'flash', 'none');
      expect(tile.headline, phase).not.toBe('');
      expect(tile.instruction, phase).not.toBe('');
    }
  });

  it('distinguishes the three ways of waiting for a removal', () => {
    // The phase alone cannot say which of these it is, which is why the last cue is an input.
    expect(tileFor('await-removal', 'run-check', 'flashed-cycle-it').headline).toBe('Flashed');
    expect(tileFor('await-removal', 'flash', 'pass').headline).toBe('Pass');
    expect(tileFor('await-removal', 'flash', 'fail').headline).toBe('Failed');
  });

  it('asks for a re-seat after a flash and a fresh board after a pass', () => {
    expect(tileFor('await-removal', 'run-check', 'flashed-cycle-it').instruction).toMatch(/re-seat/i);
    expect(tileFor('await-removal', 'flash', 'pass').instruction).toMatch(/done/i);
    expect(tileFor('idle', 'flash', 'rearmed').instruction).toMatch(/seat a board/i);
    expect(tileFor('idle', 'run-check', 'rearmed').instruction).toMatch(/again/i);
  });

  it('draws a failure and a pass in different tones', () => {
    expect(tileFor('await-removal', 'flash', 'fail').tone).toBe('error');
    expect(tileFor('await-removal', 'flash', 'pass').tone).toBe('ok');
  });

  it('says do not lift while a pass is running', () => {
    for (const phase of ['flashing', 'run-check'] as Phase[]) {
      expect(tileFor(phase, 'flash', 'busy').instruction).toMatch(/do not lift/i);
    }
  });
});

describe('soundFor', () => {
  it('makes busy a loop and everything else a one-shot', () => {
    // The held level is the point: its *absence* is what an operator notices when a contact is
    // lost mid-write.
    expect(soundFor('busy')).toEqual({ kind: 'loop' });

    const oneShots: Cue[] = ['armed', 'disarmed', 'flashed-cycle-it', 'pass', 'fail', 'rearmed'];
    for (const cue of oneShots) {
      expect(soundFor(cue).kind, cue).toBe('play');
    }
  });

  it('gives the two success cues different sounds', () => {
    // An operator who cannot hear the difference never performs the second insertion.
    const flashed = soundFor('flashed-cycle-it');
    const passed = soundFor('pass');
    expect(flashed).not.toEqual(passed);
  });

  it('reserves the failure sound for failures', () => {
    const failureSounds = (['armed', 'disarmed', 'busy', 'flashed-cycle-it', 'pass', 'rearmed'] as Cue[])
      .map(soundFor)
      .filter((action) => action.kind === 'play' && action.name === 'failure');
    expect(failureSounds).toEqual([]);
    expect(soundFor('fail')).toEqual({ kind: 'play', name: 'failure' });
  });

  it('says nothing for the initial no-cue state', () => {
    expect(soundFor('none')).toEqual({ kind: 'none' });
  });
});

describe('statusSummary', () => {
  it('reports a missing probe above everything else', () => {
    expect(statusSummary(true, 'flashing', false)).toEqual({ value: 'no probe', tone: 'error' });
  });

  it('distinguishes armed-and-working from armed-and-waiting', () => {
    expect(statusSummary(true, 'flashing', true).tone).toBe('active');
    expect(statusSummary(true, 'idle', true).tone).toBe('ok');
    expect(statusSummary(false, 'disarmed', true).tone).toBe('idle');
  });
});

describe('shortHash', () => {
  it('shortens a hash and marks an absent one', () => {
    expect(shortHash('0123456789abcdef0123')).toBe('0123456789ab');
    expect(shortHash('')).toBe('—');
  });
});
