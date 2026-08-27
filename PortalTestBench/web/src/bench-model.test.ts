import { describe, expect, it } from 'vitest';
import { type BenchView, type FirmwareItem, type LinkView, type SerialView, type SettingsView, MAX_DROP_BYTES, connectBlocker, dropVerdict, enumName, eraseWarning, firmwareRow, flashButtonLabel, linkBlocker, omitDetail, omitLabel, probeListEmpty, sortFirmware, serialState, settingsState, settingsSummary, soundFor, thresholdTone, verdictTile, whyDisabled } from './bench-model';

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

describe('soundFor', () => {
  it('holds the busy sound for a run and uses distinct terminal sounds', () => {
    expect(soundFor('run-start')).toEqual({ kind: 'loop' });
    expect(soundFor('pass')).toEqual({ kind: 'play', name: 'success' });
    expect(soundFor('fail')).toEqual({ kind: 'play', name: 'failure' });
  });

  it('makes connection informative and faults unmistakable', () => {
    expect(soundFor('connected')).toEqual({ kind: 'play', name: 'tick_small' });
    expect(soundFor('lost')).toEqual({ kind: 'play', name: 'failure' });
    expect(soundFor('attention')).toEqual({ kind: 'play', name: 'failure' });
    expect(soundFor('none')).toEqual({ kind: 'none' });
  });
});

describe('connectBlocker', () => {
  const down: LinkView = { route: 'rs485', connected: false, desired: 'rs485-serial', endpoint: 'COM15', detail: '' };

  it('refuses before a transport is chosen, and names the right noun per route', () => {
    expect(connectBlocker({ ...down, desired: 'none' })).toBe('choose a transport first');
    expect(connectBlocker({ ...down, route: 'serial', desired: 'none' })).toBe('choose a protocol first');
  });

  it('refuses an empty endpoint on transports that need one', () => {
    expect(connectBlocker({ ...down, endpoint: '' })).toBe('choose an endpoint');
    expect(connectBlocker({ ...down, route: 'serial', desired: 'vcp', endpoint: '' })).toBe('choose a port');
  });

  // The simulated module is in this process. Demanding a port for it would block Connect on a
  // field that is deliberately not rendered.
  it('lets a self-addressing transport connect with no endpoint', () => {
    expect(connectBlocker({ ...down, desired: 'sim', endpoint: '' })).toBeNull();
  });

  it('allows a fully specified link, and refuses a second connect', () => {
    expect(connectBlocker(down)).toBeNull();
    expect(connectBlocker({ ...down, connected: true })).toBe('already connected');
  });
});

describe('linkBlocker', () => {
  const down: LinkView = { route: 'rs485', connected: false, desired: 'none', endpoint: '', detail: '' };

  it('says nothing at all once the link is up', () => {
    expect(linkBlocker({ ...down, connected: true })).toBeNull();
  });

  it('names the route, so switching route changes the sentence', () => {
    expect(linkBlocker(down)).toContain('RS485');
    expect(linkBlocker({ ...down, route: 'serial' })).toContain('serial');
  });

  // A concrete failure is a better answer than generic advice, and it only became reachable
  // when Rs485Link::open stopped returning Ok for an endpoint nothing answered on.
  it('prefers the last failure over the generic advice', () => {
    const said = linkBlocker({ ...down, desired: 'rs485-tcp', endpoint: '127.0.0.1:1', detail: 'nothing answered on 127.0.0.1:1 within 750 ms' });
    expect(said).toContain('nothing answered on 127.0.0.1:1');
    expect(said).not.toContain('Pick a transport');
  });

  it('never returns a blank sentence in any not-connected state', () => {
    const states: LinkView[] = [
      down,
      { ...down, desired: 'rs485-serial' },
      { ...down, desired: 'rs485-serial', endpoint: 'COM15' },
      { ...down, route: 'serial', desired: 'vcp', endpoint: 'COM3' },
      { ...down, desired: 'sim' },
    ];
    for (const state of states) {
      expect(linkBlocker(state)?.length ?? 0).toBeGreaterThan(20);
    }
  });
});

describe('serialState', () => {
  const board: SerialView = { dbOk: true, boardPresent: true, identity: 'existing-on-board', boardSerial: 73001, entered: 73001, pending: false, dbSerial: 73001, heldBy: '' };
  const every: SerialView[] = [
    board,
    { ...board, entered: 73002 },
    { ...board, dbOk: false, entered: 73002 },
    { ...board, pending: true },
    { ...board, boardPresent: false, identity: 'unknown', boardSerial: 0 },
    { ...board, identity: 'unknown', boardSerial: 0 },
    { ...board, identity: 'corrupt', boardSerial: 0 },
    { ...board, identity: 'foreign-uid' },
    { ...board, identity: 'blank', boardSerial: 0, entered: 42, dbSerial: 0 },
    { ...board, identity: 'blank', boardSerial: 0, dbSerial: 73001 },
    { ...board, entered: 73002, heldBy: '00220016-30355107-35303836' },
  ];

  it('never returns a blank word or detail, and the detail never repeats the badge', () => {
    for (const state of every) {
      const value = serialState(state);
      expect(value.word.length).toBeGreaterThan(0);
      expect(value.detail.length).toBeGreaterThan(0);
      expect(value.detail.toLowerCase()).not.toContain(value.word.toLowerCase());
      // The hover text is a sentence about what the state means, not the word again.
      expect(value.hint.length).toBeGreaterThan(20);
    }
  });

  it('reads ON BOARD when the entered serial is the one on the board', () => {
    expect(serialState(board)).toMatchObject({ word: 'on board', detail: 'board: 73001', tone: 'ok', changed: false });
  });

  it('names the holder when the entered serial belongs to another MCU', () => {
    const clash = serialState({ ...board, entered: 73002, heldBy: '00220016-30355107-35303836' });
    expect(clash).toMatchObject({ word: 'conflict', tone: 'error' });
    expect(clash.detail).toContain('35303836');
    expect(clash.hint).toContain('00220016-30355107-35303836');
  });

  // The regression that issues one MCU two serials: a provisioned board that was later erased
  // reads `blank`, and calling that "fresh" invites accepting a brand-new number for it.
  it('distinguishes an erased board the registry knows from a genuinely fresh one', () => {
    expect(serialState({ ...board, identity: 'blank', boardSerial: 0, dbSerial: 0, entered: 42 }))
      .toMatchObject({ word: 'fresh', tone: 'idle' });
    const erased = serialState({ ...board, identity: 'blank', boardSerial: 0, dbSerial: 73001 });
    expect(erased).toMatchObject({ word: 'erased', tone: 'warn' });
    expect(erased.detail).toContain('73001');
  });

  it('reads CHANGED when they differ, and the detail names the board serial', () => {
    expect(serialState({ ...board, entered: 73002 })).toMatchObject({ word: 'changed', detail: 'board: 73001', tone: 'warn', changed: true });
  });

  it('reads FRESH on a blank board and does not call it changed', () => {
    // `dbSerial: 0` is what makes it fresh rather than erased: no flash serial AND no registry row.
    const value = serialState({ ...board, identity: 'blank', boardSerial: 0, entered: 42, dbSerial: 0 });
    expect(value.word).toBe('fresh');
    expect(value.tone).toBe('idle');
    expect(value.changed).toBe(false);
  });

  it('outranks everything with DB OFFLINE but still says what the board holds', () => {
    const value = serialState({ ...board, dbOk: false, entered: 73002 });
    expect(value.word).toBe('DB offline');
    expect(value.tone).toBe('error');
    expect(value.detail).toBe('board: 73001');
  });

  it('reads PENDING after a flash until the replug', () => {
    expect(serialState({ ...board, pending: true, entered: 73002 })).toMatchObject({ word: 'pending', tone: 'warn' });
  });

  it('says NO BOARD rather than FRESH when nothing is connected', () => {
    const value = serialState({ ...board, boardPresent: false, identity: 'unknown', boardSerial: 0 });
    expect(value.word).toBe('no board');
    expect(value.changed).toBe(false);
  });

  // The worker refuses both without an explicit override; the old card showed a green
  // "existing" for a foreign-UID board whose record serial happened to match.
  it('flags a foreign or corrupt identity for review even when the numbers agree', () => {
    expect(serialState({ ...board, identity: 'foreign-uid' })).toMatchObject({ word: 'review', tone: 'warn' });
    expect(serialState({ ...board, identity: 'corrupt', boardSerial: 0 })).toMatchObject({ word: 'review', tone: 'warn' });
  });

  it('distinguishes on board, changed and fresh rather than showing one idle badge', () => {
    const words = [serialState(board).word, serialState({ ...board, entered: 1 }).word, serialState({ ...board, identity: 'blank', boardSerial: 0 }).word];
    expect(new Set(words).size).toBe(3);
  });
});

describe('settingsState', () => {
  const stored: SettingsView = { boardPresent: true, identity: 'existing-on-board', pending: false, source: 'flash', boardCurrentMa: 150, boardRecovery: true, currentMa: 150, recovery: true, locked: false };
  const every: SettingsView[] = [
    stored,
    { ...stored, currentMa: 200 },
    { ...stored, recovery: false },
    { ...stored, source: 'defaults' },
    { ...stored, source: 'defaults', currentMa: 200 },
    { ...stored, pending: true },
    { ...stored, boardPresent: false, identity: 'unknown', boardCurrentMa: 0, boardRecovery: false },
  ];

  it('reads ON BOARD when both values match the journal', () => {
    expect(settingsState(stored)).toMatchObject({ word: 'on board', detail: 'board: 150 mA · recovery on', tone: 'ok', changed: false });
  });

  it('reads CHANGED when either value differs', () => {
    expect(settingsState({ ...stored, currentMa: 200 })).toMatchObject({ word: 'changed', tone: 'warn', changed: true, detail: 'board: 150 mA · recovery on' });
    expect(settingsState({ ...stored, recovery: false })).toMatchObject({ word: 'changed', changed: true });
  });

  it('reads DEFAULTS when the board has no journal and the entry is the firmware default', () => {
    expect(settingsState({ ...stored, source: 'defaults' })).toMatchObject({ word: 'defaults', detail: 'board: no settings stored', tone: 'idle', changed: false });
  });

  it('reads CHANGED, not DEFAULTS, when the board has no journal and the entry differs', () => {
    expect(settingsState({ ...stored, source: 'defaults', currentMa: 200 })).toMatchObject({ word: 'changed', detail: 'board: no settings stored' });
  });

  // The worker publishes 0 mA / off when there is no MCU; that must not read as an edit.
  it('says NO BOARD rather than DEFAULTS when nothing is connected', () => {
    const value = settingsState({ ...stored, boardPresent: false, identity: 'unknown', boardCurrentMa: 0, boardRecovery: false });
    expect(value.word).toBe('no board');
    expect(value.changed).toBe(false);
  });

  it('reads PENDING until the replug', () => {
    expect(settingsState({ ...stored, pending: true, currentMa: 200 })).toMatchObject({ word: 'pending', tone: 'warn' });
  });

  // The lock changes what happens on the next insertion, not the relation to this board.
  it('keeps the same word when locked and says so in the hint', () => {
    const locked = settingsState({ ...stored, currentMa: 200, locked: true });
    expect(locked.word).toBe('changed');
    expect(locked.hint).toContain('Locked');
    expect(settingsState({ ...stored, currentMa: 200 }).hint).not.toContain('Locked');
  });

  it('never returns a blank word or detail, and the detail never repeats the badge', () => {
    for (const state of every) {
      const value = settingsState(state);
      expect(value.word.length).toBeGreaterThan(0);
      expect(value.detail.length).toBeGreaterThan(0);
      expect(value.detail.toLowerCase()).not.toContain(value.word.toLowerCase());
      // The hover text is a sentence about what the state means, not the word again.
      expect(value.hint.length).toBeGreaterThan(20);
    }
  });
});

describe('settingsSummary', () => {
  it('spells a settings pair one way, for the card and the panel alike', () => {
    expect(settingsSummary(150, true)).toBe('150 mA · recovery on');
    expect(settingsSummary(250, false)).toBe('250 mA · recovery off');
  });
});

describe('sortFirmware', () => {
  const built = (id: string): FirmwareItem => ({ id, region: 'application', origin: 'built', bytes: 1, modified: 100 });
  const dropped = (id: string, modified: number): FirmwareItem =>
    ({ id, region: 'application', origin: 'dropped', label: id, bytes: 1, modified });

  /**
   * The row the operator just made is the reason they are looking at the list, and the bank's
   * scroller is 260 px tall — appended below four `.pio` builds it lands below the fold.
   */
  it('puts dropped images first, newest first, and leaves the build tree in discovery order', () => {
    const order = sortFirmware([built('a'), built('b'), dropped('old', 100), dropped('new', 200)]);
    expect(order.map((item) => item.id)).toEqual(['new', 'old', 'a', 'b']);
  });

  it('breaks a same-second tie on the id rather than on argument order', () => {
    const forwards = sortFirmware([dropped('z', 100), dropped('a', 100)]).map((i) => i.id);
    const backwards = sortFirmware([dropped('a', 100), dropped('z', 100)]).map((i) => i.id);
    expect(forwards).toEqual(backwards);
  });
});

describe('dropVerdict', () => {
  it('lets a .bin and a .elf through', () => {
    expect(dropVerdict('firmware.bin', 60_000)).toBeNull();
    expect(dropVerdict('firmware.elf', 158_788)).toBeNull();
    expect(dropVerdict('FIRMWARE.BIN', 60_000)).toBeNull();
  });

  it('names the extension it got when it is not one of those', () => {
    expect(dropVerdict('holiday.mp4', 60_000)).toContain('mp4');
    expect(dropVerdict('firmware.hex', 60_000)).toContain('drop a .bin or a .elf');
  });

  /**
   * The one thing the host genuinely cannot do for itself: it has no idea what was dragged until
   * the body arrives, and sending eight hundred megabytes up a loopback socket to be told it is
   * not firmware is a poor way to find out.
   */
  it('refuses something far too large before it is uploaded', () => {
    expect(dropVerdict('firmware.bin', MAX_DROP_BYTES + 1)).toContain('too large');
    expect(dropVerdict('firmware.bin', 0)).toContain('empty');
  });
});

describe('firmwareRow', () => {
  const optical: FirmwareItem = { id: 'portalfw:application_bank_optical', region: 'application', origin: 'built', bytes: 98196, modified: 1787646875, variant: 'optical', hardware: 'PCB v6', banner: 'Portal v2026-08-25_17.34 8799276+' };
  const dropped: FirmwareItem = { id: 'dropped:a1b2c3d4e5f6', region: 'application', origin: 'dropped', label: 'rc3 optical.bin', bytes: 98196, modified: 1787646875, banner: 'Portal v2026-08-25_17.34 8799276+' };
  const built: FirmwareItem = { id: 'portalbootloader:bootloader', region: 'bootloader', origin: 'built', bytes: 19568, modified: 1787647255, banner: 'Bootloader v5' };
  const reference: FirmwareItem = { id: 'reference:BootloaderRS485-2023-08-26.bin', region: 'bootloader', origin: 'reference', bytes: 22708, modified: 1787142932, banner: 'Bootloader v4' };

  /**
   * A dropped image is named by the file. It is the only name it has -- nothing in a `.bin` says
   * which PCB it targets -- and it is the name the operator chose, so it is the one they will
   * recognise in the picker a minute later.
   */
  it('names a dropped image by its file, and says when it arrived rather than when it was built', () => {
    const row = firmwareRow(dropped);
    expect(row.title).toBe('rc3 optical.bin');
    expect(row.detail).toContain('dropped ');
    expect(row.detail).not.toContain('built ');
    expect(row.detail).toContain('95.9 kB');
  });

  it('falls back to a name rather than an empty title for a dropped image with no label', () => {
    expect(firmwareRow({ ...dropped, label: undefined }).title).toBe('Dropped file');
  });

  it('names the PCB in the title and the build in the detail, without repeating the product', () => {
    const row = firmwareRow(optical);
    expect(row.title).toBe('Optical · PCB v6');
    expect(row.detail).toContain('v2026-08-25_17.34 8799276+');
    expect(row.detail).not.toContain('Portal v');
    expect(row.detail).toContain('built ');
    expect(row.detail).toContain('95.9 kB');
  });

  it('names a built bootloader by its banner and a reference image by its file, not its mtime', () => {
    expect(firmwareRow(built)).toMatchObject({ title: 'Built from source' });
    expect(firmwareRow(built).detail).toContain('v5');
    const ref = firmwareRow(reference);
    expect(ref.title).toBe('Reference image');
    expect(ref.detail).toContain('BootloaderRS485-2023-08-26.bin');
    expect(ref.detail).toContain('v4');
    expect(ref.detail).not.toContain('built ');
  });

  it('still lists an image whose banner could not be read', () => {
    const row = firmwareRow({ ...optical, banner: null });
    expect(row.detail).toContain('95.9 kB');
    expect(row.detail).not.toContain('null');
    expect(row.detail).not.toContain('undefined');
  });
});

describe('the ST-Link list when it is empty', () => {
  const attached = { loaded: true, error: '', probeConnected: false, probeName: '', swdSupport: true };

  /**
   * The complaint this whole change answers: "No ST-Link found. Connect the fixture." on a bench
   * that was flashing a board at the time. Whatever else this function says, it must never say
   * that while the worker has a probe open.
   */
  it('never claims no ST-Link while one is open', () => {
    const open = probeListEmpty({ ...attached, probeConnected: true, probeName: 'ST-Link V2-1' });
    expect(open.detail).not.toContain('No ST-Link found');
    expect(open.detail).toContain('ST-Link V2-1');
  });

  it('distinguishes not-yet-read from nothing-attached', () => {
    expect(probeListEmpty({ ...attached, loaded: false }).detail).toContain('Reading');
    expect(probeListEmpty(attached).detail).toBe('No ST-Link found. Connect the fixture probe and rescan.');
  });

  it('says a failed ask failed, rather than reporting an empty bench', () => {
    const failed = probeListEmpty({ ...attached, error: 'the bench answered 503' });
    expect(failed.detail).toContain('503');
    expect(failed.detail).not.toContain('No ST-Link found');
  });

  /** The flag exists to stop an empty list reading as "none attached"; it was never read. */
  it('says so when the build cannot see probes at all', () => {
    const blind = probeListEmpty({ ...attached, swdSupport: false });
    expect(blind.detail).toContain('not evidence');
    expect(blind.detail).not.toContain('No ST-Link found');
  });
});

describe('leaving a bank out', () => {
  it('names the choice per bank', () => {
    // Each label names an outcome, not a selection. "No bootloader" named neither, and left the
    // operator to find a switch elsewhere on the page before they could know what they had chosen.
    expect(omitLabel('bootloader', 'keep')).toBe('Keep existing bootloader');
    expect(omitLabel('bootloader', 'erase')).toBe('Erase bootloader bank');
    expect(omitLabel('application', 'keep')).toBe('Keep existing application');
    expect(omitLabel('application', 'erase')).toBe('Erase application bank');
  });

  // Both are offered at once, side by side, precisely because they are opposites: the operator
  // picks the outcome rather than picking "not this one" and then qualifying it.
  it('says what each of the two does, and the two answers are opposites', () => {
    expect(omitDetail('application', 'keep')).toContain('read back');
    expect(omitDetail('bootloader', 'keep')).toContain('Left untouched');
    expect(omitDetail('application', 'erase')).toContain('no application to run');
    expect(omitDetail('bootloader', 'erase')).toContain('will not be able to boot');
  });

  it('warns about an erase, and says plainly when it costs the board its ability to boot', () => {
    const boot = eraseWarning(true, false);
    expect(boot).toContain('bootloader bank');
    expect(boot).toContain('will not be able to boot');

    const app = eraseWarning(false, true);
    expect(app).toContain('application bank');
    expect(app).not.toContain('will not be able to boot');

    expect(eraseWarning(true, true)).toContain('nothing to flash');
  });

  // A one-bank pass pressed by an operator who thought they were doing a full one is the whole
  // hazard this label exists to remove.
  it('never calls a one-bank pass a full provision', () => {
    expect(flashButtonLabel('full')).toBe('Flash / Provision now');
    expect(flashButtonLabel('bootloader only')).toBe('Flash bootloader only');
    expect(flashButtonLabel('application only')).toBe('Flash application only');
    expect(flashButtonLabel('nothing')).toBe('Nothing selected');
  });
});
