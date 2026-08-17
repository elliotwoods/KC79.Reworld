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
  stepSummary,
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
    step: 'idle',
    stepFraction: 0,
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
    expect(tileFor(state({ targetPresent: false })).headline).toBe('No board');
    expect(tileFor(state({ targetPresent: false })).instruction).toMatch(/seat a board/i);
    expect(tileFor(state({ targetPresent: true })).instruction).toMatch(/flash now/i);
  });

  it('leads with the board, not with what is missing elsewhere', () => {
    // "No image" as a headline read as the status panel complaining about something the firmware
    // panel was already saying, and told an operator nothing about whether their board was even
    // detected. What is missing belongs in the instruction.
    const tile = tileFor(state({ targetPresent: true, hasImage: false }));
    expect(tile.headline).toBe('Board detected');
    expect(tile.instruction).toMatch(/choose firmware/i);
  });

  it('reports no board before it reports no image', () => {
    // With neither, the board is the more useful thing to say: an operator can seat one, whereas
    // "no image" is the firmware panel's business.
    expect(tileFor(state({ targetPresent: false, hasImage: false })).headline).toBe('No board');
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

describe('stepSummary', () => {
  it('says nothing between passes', () => {
    expect(stepSummary(state())).toBeNull();
    // `busy` on its own is not a step: the pass may not have reported one yet.
    expect(stepSummary(state({ busy: true }))).toBeNull();
  });

  it('marks exactly the stages where lifting the board leaves it half-written', () => {
    // The distinction this whole function exists for. Attach and the option-byte write are both
    // recoverable -- nothing in flash has changed yet -- and the readback is a pure read, so a
    // board lifted during it is already fully programmed and verified up to that point.
    expect(stepSummary(state({ step: 'attach' }))?.committed).toBe(false);
    expect(stepSummary(state({ step: 'option-bytes' }))?.committed).toBe(false);
    expect(stepSummary(state({ step: 'erase' }))?.committed).toBe(true);
    expect(stepSummary(state({ step: 'program' }))?.committed).toBe(true);
    expect(stepSummary(state({ step: 'readback' }))?.committed).toBe(false);
    expect(stepSummary(state({ step: 'reset-run' }))?.committed).toBe(false);
  });

  it('shows a percentage only where one means something', () => {
    // Erase, program and readback each move through the whole part. Attach and reset-run are
    // single events, and "attach 0%" would suggest a progress bar that is stuck.
    expect(stepSummary(state({ step: 'program', stepFraction: 0.5 }))?.label).toBe('program 50%');
    expect(stepSummary(state({ step: 'attach', stepFraction: 0 }))?.label).toBe('attach');
    expect(stepSummary(state({ step: 'reset-run', stepFraction: 1 }))?.label).toBe('reset-run');
  });

  it('rounds rather than truncating, so a finished stage reads as finished', () => {
    expect(stepSummary(state({ step: 'erase', stepFraction: 0.999 }))?.label).toBe('erase 100%');
    expect(stepSummary(state({ step: 'erase', stepFraction: 1 }))?.label).toBe('erase 100%');
  });
});
