import { describe, expect, it } from 'vitest';

import {
  type Cue,
  type Layout,
  type Phase,
  type RigState,
  flashNowState,
  layoutSummary,
  readDeviceState,
  shortHash,
  soundFor,
  statusSummary,
  tileFor,
} from './flasher-model';

function state(over: Partial<RigState> = {}): RigState {
  return {
    mode: 'manual',
    armed: false,
    phase: 'disarmed',
    expect: 'flash',
    lastCue: 'none',
    probeConnected: true,
    targetPresent: false,
    busy: false,
    hasImage: true,
    ...over,
  };
}

describe('tileFor', () => {
  it('always says what to physically do', () => {
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
      for (const mode of ['manual', 'auto'] as const) {
        const tile = tileFor(state({ mode, phase }));
        expect(tile.headline, `${mode}/${phase}`).not.toBe('');
        expect(tile.instruction, `${mode}/${phase}`).not.toBe('');
      }
    }
  });

  it('reports a missing probe above everything else', () => {
    // Every other state is meaningless without one, including a phase that says "flashing".
    const tile = tileFor(state({ probeConnected: false, mode: 'auto', phase: 'flashing' }));
    expect(tile.headline).toBe('No probe');
    expect(tile.tone).toBe('error');
  });

  it('manual asks for a board, then offers to flash it', () => {
    expect(tileFor(state({ targetPresent: false })).instruction).toMatch(/seat a board/i);
    expect(tileFor(state({ targetPresent: true })).instruction).toMatch(/flash now/i);
  });

  it('manual will not offer to flash without an image', () => {
    const tile = tileFor(state({ targetPresent: true, hasImage: false }));
    expect(tile.headline).toBe('No image');
    expect(tile.instruction).toMatch(/choose firmware/i);
  });

  it('auto distinguishes the three ways of waiting for a removal', () => {
    const auto = { mode: 'auto', phase: 'await-removal' } as const;
    expect(tileFor(state({ ...auto, expect: 'run-check', lastCue: 'flashed-cycle-it' })).headline)
      .toBe('Flashed');
    expect(tileFor(state({ ...auto, lastCue: 'pass' })).headline).toBe('Pass');
    expect(tileFor(state({ ...auto, lastCue: 'fail' })).headline).toBe('Failed');
  });

  it('says do not lift while anything is being written', () => {
    expect(tileFor(state({ busy: true })).instruction).toMatch(/do not lift/i);
    expect(tileFor(state({ mode: 'auto', phase: 'flashing' })).instruction).toMatch(/do not lift/i);
    expect(tileFor(state({ mode: 'auto', phase: 'run-check' })).instruction).toMatch(/do not lift/i);
  });
});

describe('flashNowState', () => {
  it('is offered only when everything it needs is true', () => {
    expect(flashNowState(state({ targetPresent: true })).enabled).toBe(true);
  });

  it('is refused while auto-flash is armed, and says so', () => {
    // The two paths must never run at once: the machine owns the probe during a pass.
    const refused = flashNowState(state({ mode: 'auto', targetPresent: true }));
    expect(refused.enabled).toBe(false);
    expect(refused.reason).toMatch(/auto-flash/);
  });

  it('gives a specific reason for every refusal', () => {
    expect(flashNowState(state({ probeConnected: false })).reason).toMatch(/probe/);
    expect(flashNowState(state({ targetPresent: true, hasImage: false })).reason).toMatch(/image/);
    expect(flashNowState(state({ targetPresent: false })).reason).toMatch(/fixture/);
    expect(flashNowState(state({ targetPresent: true, busy: true })).reason).toMatch(/busy/);
  });
});

describe('readDeviceState', () => {
  it('needs no image, because reading is not flashing', () => {
    expect(readDeviceState(state({ targetPresent: true, hasImage: false })).enabled).toBe(true);
  });

  it('still needs a probe and a board', () => {
    expect(readDeviceState(state({ targetPresent: false })).enabled).toBe(false);
    expect(readDeviceState(state({ probeConnected: false })).enabled).toBe(false);
  });
});

describe('soundFor', () => {
  it('makes busy a loop and everything else a one-shot', () => {
    expect(soundFor('busy')).toEqual({ kind: 'loop' });
    const oneShots: Cue[] = ['armed', 'disarmed', 'flashed-cycle-it', 'pass', 'fail', 'rearmed'];
    for (const cue of oneShots) {
      expect(soundFor(cue).kind, cue).toBe('play');
    }
  });

  it('gives the two success cues different sounds', () => {
    // An operator who cannot hear the difference never performs the second insertion.
    expect(soundFor('flashed-cycle-it')).not.toEqual(soundFor('pass'));
  });

  it('reserves the failure sound for failures', () => {
    const wrong = (['armed', 'disarmed', 'busy', 'flashed-cycle-it', 'pass', 'rearmed'] as Cue[])
      .map(soundFor)
      .filter((a) => a.kind === 'play' && a.name === 'failure');
    expect(wrong).toEqual([]);
    expect(soundFor('fail')).toEqual({ kind: 'play', name: 'failure' });
  });
});

describe('layoutSummary', () => {
  it('describes every layout the Rust side can report', () => {
    const layouts: Layout[] = ['unknown', 'erased', 'split', 'flat', 'unrecognised'];
    for (const layout of layouts) {
      const summary = layoutSummary(layout);
      expect(summary.label, layout).not.toBe('');
      expect(summary.detail, layout).not.toBe('');
    }
  });

  it('calls a flat image out as unable to take a field update', () => {
    // This is the board actually on the bench, and the fact an operator most needs: it runs, but
    // the RS485 updater has no bootloader to talk to.
    const flat = layoutSummary('flat');
    expect(flat.detail).toMatch(/no_bootloader/);
    expect(flat.detail).toMatch(/field update/);
    expect(flat.tone).not.toBe('ok');
  });

  it('treats the split arrangement as the production one', () => {
    expect(layoutSummary('split').tone).toBe('ok');
  });
});

describe('statusSummary', () => {
  it('reports a missing probe above everything else', () => {
    expect(statusSummary(state({ probeConnected: false }))).toEqual({
      value: 'no probe',
      tone: 'error',
    });
  });

  it('names the mode, and the phase only when it means something', () => {
    expect(statusSummary(state()).value).toBe('manual');
    expect(statusSummary(state({ busy: true })).value).toMatch(/flashing/);
    expect(statusSummary(state({ mode: 'auto', phase: 'idle' })).value).toBe('auto · idle');
  });
});

describe('shortHash', () => {
  it('shortens a hash and marks an absent one', () => {
    expect(shortHash('0123456789abcdef0123')).toBe('0123456789ab');
    expect(shortHash('')).toBe('—');
  });
});
