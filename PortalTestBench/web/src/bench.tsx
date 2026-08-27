import {
  Badge, Banner, Button, EmptyState, EnumSelect, NumberField, Panel, ParamTree, Row, StatusBar, StatusItem,
  Tabs, TextField, TitleBar, Toggle,
} from '@auroravision/av-gui/controls';
import { SystemSounds } from '@auroravision/av-gui/calibration';
import { mount, useParam, useSchema } from '@auroravision/av-gui/runtime';
import '@auroravision/av-gui/styles.css';
import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { type BoardValue, type Cue, type FirmwareItem, type LinkView, type SerialView, type SettingsView, connectBlocker, eraseWarning, firmwareRow, flashButtonLabel, linkBlocker, omitDetail, omitLabel, probeListEmpty, sortFirmware, serialState, settingsState, settingsSummary, soundFor } from './bench-model';
import { loadSurvey, loadSurveyForGeneration, subscribeSurvey, surveySnapshot, type ProbeChoice } from './bench-survey';
import { SessionLog } from './bench-log';
import { MotionGraphs, MotionPilot } from './motion';
import { InspectTab } from './inspect';
import { FirmwareDrop, FirmwareDropStrip } from './firmware-drop';
import { DatabaseTab } from './database';

function useEnumName(path: string): string {
  const p = useParam<number>(path);
  return p.decl?.variants.find((v) => v.value === p.value)?.name ?? 'unknown';
}
const useText = (path: string) => useParam<string>(path).value ?? '';
const useNumber = (path: string) => useParam<number>(path).value ?? 0;
const useBool = (path: string) => !!useParam<boolean>(path).value;

/** The reads behind the SERIAL NUMBER card, shared with the provisioning panel so the two cannot disagree. */
function useSerialView(): SerialView {
  return {
    dbOk: useBool('/provision/database_ok'),
    boardPresent: useBool('/probe/target_present'),
    identity: useText('/provision/identity_state'),
    boardSerial: useNumber('/provision/on_board_serial'),
    entered: useNumber('/provision/serial_to_provision'),
    pending: useBool('/provision/pending_replug'),
    dbSerial: useNumber('/provision/database_serial'),
    heldBy: useText('/provision/entered_serial_holder'),
  };
}

/** The reads behind the MODULE SETTINGS card, likewise. */
function useSettingsView(): SettingsView {
  return {
    boardPresent: useBool('/probe/target_present'),
    identity: useText('/provision/identity_state'),
    pending: useBool('/provision/pending_replug'),
    source: useText('/provision/settings/source'),
    boardCurrentMa: useNumber('/provision/settings/on_board_current_ma'),
    boardRecovery: useBool('/provision/settings/on_board_recovery'),
    currentMa: useNumber('/provision/settings/current_ma'),
    recovery: useBool('/provision/settings/recovery_enabled'),
    locked: useBool('/provision/settings/locked'),
  };
}

function Action({ path, children, why, variant, className }: { path: string; children: ReactNode; why?: string | null; variant?: 'default' | 'primary' | 'danger' | 'quiet'; className?: string }) {
  const p = useParam<number>(path);
  const disabled = !!why || !p.decl;
  return <span className={className} title={why ?? p.decl?.label ?? path}><Button variant={variant} disabled={disabled} onClick={() => p.set((p.value ?? 0) + 1)}>{children}</Button></span>;
}

function Fact({ label, value, tone }: { label: string; value: ReactNode; tone?: string }) {
  return <div className={`fact${tone ? ` is-${tone}` : ''}`}><span className="fact-label">{label}</span><span className="fact-value">{value}</span></div>;
}

/**
 * `hide` exists for one variant: `sim`.
 *
 * The enum has to carry it — the worker maps the bus value back to `LinkKind::Sim`, and a
 * variant that is absent from the table silently becomes 0 — but a production bench must not
 * offer a transport it cannot open. `schema.rs` states the rule for parameters ("the production
 * schema never carries dead controls"); this is the same rule one level down, for a variant.
 */
function FriendlyEnum({ path, labels, hide }: { path: string; labels: Record<string, string>; hide?: string[] }) {
  const p = useParam<number>(path);
  const variants = (p.decl?.variants ?? []).filter((v) => !hide?.includes(v.name));
  return <select className="friendly-select" value={p.value ?? 0} disabled={!p.decl || p.readOnly} onChange={(e) => p.set(Number(e.target.value))} aria-label={p.decl?.label ?? path}>
    {variants.map((v) => <option key={v.value} value={v.value}>{labels[v.name] ?? v.name}</option>)}
  </select>;
}

function HardwareBand() {
  const probe = useBool('/probe/connected');
  const target = useBool('/probe/target_present');
  const dut = useBool('/dut/present');
  const mcuFirmware = useText('/mcu/firmware');
  const dutVersion = useText('/dut/version');
  const probeName = useText('/probe/name');
  const probeFirmware = useText('/probe/firmware');
  const mcuPart = useText('/mcu/part');
  const mcuUid = useText('/mcu/uid');
  const mcuIdcode = useText('/mcu/idcode');
  const mcuDevId = useText('/mcu/dev_id');
  const flashKb = useNumber('/mcu/flash_kb');
  const mcuLayout = useText('/mcu/layout');
  const mcuRdp = useText('/mcu/rdp');
  const bootState = useText('/flash/boot_state');
  const visibleBootState = bootState === 'running' || bootState === 'checking' ? bootState : '—';
  return <section className="hardware-band" data-av-surface="module-state">
    <div className="hardware-status"><span className={`presence-dot ${probe && target ? 'is-ok' : 'is-warn'}`} /><div><strong>{target ? 'MCU connected' : probe ? 'ST-Link ready · no target' : 'No ST-Link'}</strong><small>{dut ? 'communications online' : target ? 'target responding' : 'waiting for hardware'}</small></div></div>
    <div className="hardware-facts">
      <Fact label="Firmware" value={mcuFirmware || dutVersion || '—'} />
      <Fact label="ST-Link" value={[probeName, probeFirmware].filter(Boolean).join(' · ') || '—'} />
      <Fact label="MCU / Flash" value={[mcuPart, flashKb > 0 ? `${flashKb} kB` : '', mcuLayout, mcuRdp ? `RDP ${mcuRdp}` : ''].filter(Boolean).join(' · ') || '—'} />
      <Fact label="UID / IDs" value={[mcuUid, mcuIdcode && `IDCODE ${mcuIdcode}`, mcuDevId && `DEV_ID ${mcuDevId}`].filter(Boolean).join(' · ') || '—'} />
      <Fact label="Boot" value={visibleBootState} />
    </div>
  </section>;
}

interface Artefact extends FirmwareItem { label: string; fits: boolean }
interface MissingArtefact { label: string; path: string; hint: string }

/**
 * The shared hardware survey, re-read whenever the worker says it moved.
 *
 * The document and the reasons it is one document live in `bench-survey.ts`. All this adds is the
 * subscription: `/setup/ports_generation` is bumped by the worker only when the set of attached
 * devices actually changes, or when somebody rescans — so this re-fetches exactly when there is
 * something new to fetch, including on the first render, and never otherwise.
 */
function usePortSurvey() {
  const [, bump] = useState(0);
  const generation = useNumber('/setup/ports_generation');
  useEffect(() => subscribeSurvey(() => bump((n) => n + 1)), []);
  useEffect(() => { loadSurveyForGeneration(generation); }, [generation]);
  return { ...surveySnapshot(), refresh: loadSurvey };
}

/**
 * One row of a picker.
 *
 * `empty` is the "write nothing to this bank" row, which is a choice like any other and must not
 * look like one of the images: it is drawn hollow -- dashed, muted, a hatched mark -- so a glance
 * at the two banks says which of them has an image in it.
 *
 * Its two values are not decoration. `keep` leaves the board's firmware alone; `erase` destroys it,
 * and for the bootloader bank that costs the board its ability to start. Same row, same click,
 * opposite outcomes -- so the row that means "erase" is tinted like one, and the operator does not
 * have to read the switch below to know which one is armed.
 */
function ChoiceRow({ selected, disabled = false, empty, title, detail, badges, onClick }: { selected: boolean; disabled?: boolean; empty?: 'keep' | 'erase'; title: string; detail: string; badges?: ReactNode; onClick: () => void }) {
  return <button type="button" role="option" aria-selected={selected} data-selected={selected} data-empty={empty} data-disabled={disabled || undefined} className="choice-row" disabled={disabled} onClick={onClick}>
    <span className="choice-mark" aria-hidden="true">{selected ? '✓' : ''}</span>
    <span className="choice-copy"><strong>{title}</strong><small>{detail}</small></span>
    {badges && <span className="choice-badges">{badges}</span>}
  </button>;
}

function SetupPicker() {
  const boot = useParam<string>('/flash/boot_id');
  const app = useParam<string>('/flash/app_id');
  const probe = useParam<string>('/probe/selected');
  const rescan = useParam<number>('/actions/rescan_firmware');
  const simulated = useBool('/setup/simulated');
  const connected = useBool('/probe/connected');
  const armed = useBool('/flash/armed');
  const flashBusy = useBool('/flash/busy');
  const runBusy = useBool('/run/busy');
  const scope = useText('/flash/scope');
  const preserveParam = useParam<boolean>('/flash/preserve_unselected');
  const preserve = preserveParam.value !== false;
  const probeName = useText('/probe/name');
  const generation = useNumber('/setup/ports_generation');
  // The firmware list has its own counter. A drop changes the list and nothing else, and bumping
  // the hardware one for it would drag a full port survey along behind it.
  const artefactsGeneration = useNumber('/flash/artefacts_generation');
  const { probes, swd_support: swdSupport, loaded, loading: surveying, error } = usePortSurvey();
  const [items, setItems] = useState<Artefact[]>([]);
  const [missing, setMissing] = useState<MissingArtefact[]>([]);
  const [root, setRoot] = useState('');
  const [loadingFirmware, setLoadingFirmware] = useState(false);
  const loading = loadingFirmware || surveying;
  // Firmware only: the survey refreshes itself off the generation.
  const load = async () => {
    setLoadingFirmware(true);
    try {
      const firmwareResponse = await fetch('/api/bench/firmware', { cache: 'no-store' });
      if (firmwareResponse.ok) {
        const firmware = await firmwareResponse.json();
        setItems(firmware.found ?? []);
        setMissing(firmware.missing ?? []);
        setRoot(firmware.root ?? '');
      }
    } catch {
      // The host may be restarting; the explicit rescan and the next page load retry.
    } finally {
      setLoadingFirmware(false);
    }
  };
  // Removing a staged image is a request like any other: the worker owns the picker's state, so
  // this enqueues and the list refreshes on the generation bump rather than being edited here.
  const forget = async (id: string) => {
    try {
      await fetch(`/api/bench/firmware/dropped?id=${encodeURIComponent(id)}`, { method: 'DELETE' });
    } catch {
      // Same reasoning as `load`: the next generation bump or page load corrects it.
    }
  };
  // A rescan re-reads the build tree as well as the hardware, and it announces on the generation
  // whether or not anything moved — so this covers both front doors, and an agent's rescan too.
  useEffect(() => { void load(); }, [generation, artefactsGeneration]);
  const probeChoices: ProbeChoice[] = simulated
    ? [{ identifier: 'sim', name: 'SimRig', serial_number: 'SIM', kind: 'simulation' }]
    : probes;
  const selectedProbeMissing = !!probe.value && !probeChoices.some((item) => item.identifier === probe.value);
  const setupLocked = armed || flashBusy || runBusy;
  // Just the counter bump. The worker answers it by rescanning and announcing on the generation,
  // which is what refreshes this page — so there is nothing to time out and race any more.
  const doRescan = () => rescan.set((rescan.value ?? 0) + 1);
  return <div className="setup-picker">
    <div className="setup-picker-toolbar">
      <div><strong>Fixture setup</strong><small>Choose hardware first, then the image banks to program.</small></div>
      <Button variant="quiet" disabled={loading || setupLocked} onClick={doRescan}>{loading ? 'Scanning…' : 'Rescan all'}</Button>
    </div>
    <div className="setup-picker-columns">
      <section className="setup-choice-group" aria-label="Probe selection">
        <header><span><b>1</b> ST-Link probe</span><Badge tone={connected ? 'ok' : 'offline'}>{connected ? 'connected' : 'not connected'}</Badge></header>
        {probeChoices.length === 0 ? <EmptyState inline {...probeListEmpty({ loaded, error, probeConnected: connected, probeName, swdSupport })} /> :
          <div className="choice-list" role="listbox" aria-label="ST-Link probes">{probeChoices.map((item) =>
            <ChoiceRow key={item.identifier} selected={probe.value === item.identifier} disabled={setupLocked} title={item.name || item.identifier} detail={[item.kind, item.serial_number || item.identifier].filter(Boolean).join(' · ')} onClick={() => probe.set(item.identifier)} />
          )}</div>}
        {probeChoices.length > 1 && !probe.value && <Banner tone="warn">Multiple probes are attached. Select the one wired to this fixture; the bench will not guess.</Banner>}
        {selectedProbeMissing && <Banner tone="warn">The selected probe is no longer attached. Rescan or choose another.</Banner>}
        <footer>{useText('/probe/name') || 'No probe open'}<span>{useText('/probe/firmware') || '—'} · {useNumber('/probe/speed_khz') || 0} kHz</span></footer>
      </section>
      <section className="setup-choice-group" aria-label="Firmware selection">
        <header><span><b>2</b> Firmware banks</span></header>
        {items.length === 0 ? <EmptyState inline detail="No production firmware was found in the build tree." /> :
          <div className="firmware-banks">{(['bootloader', 'application'] as const).map((region) => {
            const selection = region === 'bootloader' ? boot : app;
            const regionItems = sortFirmware(items.filter((item) => item.region === region));
            const omitted = !selection.value;
            return <section key={region} className="firmware-bank" aria-label={`${region} bank`}>
              <strong>{region === 'bootloader' ? 'PortalBootloader' : 'PortalFW Application'}</strong>
              {regionItems.length === 0 ? <small>No {region} image found.</small> : <div className="choice-list" role="listbox" aria-label={`${region} firmware`}>
                {/* Not writing this bank is two outcomes, not one, so it is two rows sitting with
                    the images as peers. Picking one empties the bank *and* sets the policy, which
                    is why the "preserve the bank left out" switch that used to be below is gone:
                    the rows are that switch, said where the decision is being made. */}
                {(['keep', 'erase'] as const).map((kind) =>
                  <ChoiceRow key={kind} empty={kind} selected={omitted && preserve === (kind === 'keep')} disabled={setupLocked} title={omitLabel(region, kind)} detail={omitDetail(region, kind)} onClick={() => { preserveParam.set(kind === 'keep'); selection.set(''); }} />
                )}
                {regionItems.map((item) => {
                  const selected = selection.value === item.id;
                  const row = firmwareRow(item);
                  const badges = <>
                    {item.origin === 'dropped' && <Badge tone="active">dropped</Badge>}
                    {!item.fits && <Badge tone="error">too large</Badge>}
                  </>;
                  const choice = <ChoiceRow key={item.id} selected={selected} disabled={!item.fits || setupLocked} title={row.title} detail={row.detail} badges={badges} onClick={() => selection.set(selected ? '' : item.id)} />;
                  // A remove button cannot live inside `ChoiceRow` -- that is a `<button>`, and a
                  // button inside a button is invalid HTML that browsers resolve by dropping one of
                  // them. So it is a sibling, and the pair share a row.
                  return item.origin === 'dropped'
                    ? <div className="choice-row-with-action" key={item.id}>
                        {choice}
                        <button type="button" className="choice-row-remove" disabled={setupLocked} title={`Forget ${row.title}`} aria-label={`Forget ${row.title}`} onClick={() => void forget(item.id)}>×</button>
                      </div>
                    : choice;
                })}
              </div>}
            </section>;
          })}</div>}
        <FirmwareDropStrip disabled={setupLocked} />
        {/* The one control on this page that can destroy working firmware. It should not be quiet. */}
        {!preserve && (!boot.value || !app.value) && <Banner tone="warn">{eraseWarning(!boot.value, !app.value)}</Banner>}
        {setupLocked && <Banner tone="info">Selection is locked while flashing, auto-flash, or a test plan is active.</Banner>}
        {missing.map((item) => <Banner key={item.path} tone="info">{item.label}: {item.hint}</Banner>)}
        <footer title={root}><span>{scope || 'nothing'} selected</span><code>{root || 'build tree unavailable'}</code></footer>
      </section>
    </div>
  </div>;
}

function ManualFlashButton() {
  const action = useParam<number>('/actions/flash_now');
  const busy = useBool('/flash/busy');
  const armed = useBool('/flash/armed');
  const probe = useBool('/probe/connected');
  const target = useBool('/probe/target_present');
  const runBusy = useBool('/run/busy');
  const scope = useText('/flash/scope');
  const [confirmUntil, setConfirmUntil] = useState(0);
  const why = busy ? 'a flash pass is running' : runBusy ? 'a test plan is running' : armed ? 'disable auto-flash first' : !probe ? 'no ST-Link' : !target ? 'no MCU is answering' : scope === 'nothing' ? 'no firmware selected' : null;
  const click = () => {
    const now = Date.now();
    if (now < confirmUntil) { action.set((action.value ?? 0) + 1); setConfirmUntil(0); }
    else { setConfirmUntil(now + 5000); window.setTimeout(() => setConfirmUntil((v) => v <= Date.now() ? 0 : v), 5100); }
  };
  return <span className="manual-flash" title={why ?? 'Press twice within five seconds. Always programs and restarts the board, even if it already carries this image.'}><Button variant={confirmUntil > Date.now() ? 'danger' : 'primary'} disabled={!!why || !action.decl} onClick={click}>{confirmUntil > Date.now() ? 'Confirm flash' : flashButtonLabel(scope)}</Button></span>;
}

/**
 * One card in the action strip: an entered value held against what the board holds. The field is
 * what was entered, the small line is what the board holds, the badge is the relation — three
 * facts, none repeated. The tint follows the badge tone, so a card that needs attention looks it.
 */
function BoardValueCard({ kind, title, hint, state, trailing, children }: { kind: 'serial' | 'settings'; title: string; hint: string; state: BoardValue; trailing?: ReactNode; children: ReactNode }) {
  return <div className="board-value" data-kind={kind} data-tone={state.tone} role="group" aria-label={title}>
    <span className="board-value-copy" title={hint}><span className="label-caps">{title}</span><small>{state.detail}</small></span>
    {children}
    <Badge tone={state.tone} title={state.hint}>{state.word}</Badge>
    {trailing}
  </div>;
}

const SERIAL_HINT = 'The serial to provision: written to the protected identity page on the next flash. Durable, and separate from the 1–127 RS485 address.';
const SETTINGS_HINT = 'Stored in the module\'s flash settings journal, written with the serial on the next flash.';
const CURRENT_HINT = 'Operating current: the motor driver current the module runs at, 50–250 mA.';
const RECOVERY_HINT = 'Full-current home recovery: if a home fails for a motion reason at the stored current, retry it once at 250 mA — and if that succeeds, promote the stored current to 250 mA.';
const LOCK_HINT = 'Lock: keep the entered settings when a board is connected, instead of replacing them with what that board holds. Set a value once, lock it, and every flash writes it. Read from board is disabled while locked.';

/** The padlock at the right of the settings card. Locked, the entered pair survives a board insertion. */
function SettingsLock() {
  const p = useParam<boolean>('/provision/settings/locked');
  const on = !!p.value;
  return <button type="button" className={`board-value-lock${on ? ' is-on' : ''}`} aria-pressed={on} aria-label={on ? 'Unlock module settings' : 'Lock module settings'} title={LOCK_HINT} disabled={!p.decl} onClick={() => p.set(!on)}>
    <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><rect x="3" y="7.5" width="10" height="6.5" rx="1.5" fill="currentColor" /><path d={on ? 'M5 7.5V5a3 3 0 0 1 6 0v2.5' : 'M5 7.5V5a3 3 0 0 1 6 0'} fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" /></svg>
  </button>;
}

function SerialNumberCard() {
  const state = serialState(useSerialView());
  // The three sources sit inside the card, between the number and the verdict: they are what the
  // number in the box could be, so putting them anywhere else makes them a separate subject.
  return <BoardValueCard kind="serial" title="Serial Number" hint={SERIAL_HINT} state={state} trailing={<SerialSources />}>
    <span className="board-value-hint" title={SERIAL_HINT}><NumberField path="/provision/serial_to_provision" /></span>
  </BoardValueCard>;
}

// The in-flash settings go to the board in the same pass as the serial, which is why they sit
// beside it. "mA" is the field's own unit suffix; the toggle already carries its full name as an
// aria-label, so the caption under it is for the eye only.
function ModuleSettingsCard() {
  const state = settingsState(useSettingsView());
  return <BoardValueCard kind="settings" title="Module Settings" hint={SETTINGS_HINT} state={state} trailing={<SettingsLock />}>
    <span className="board-value-hint" title={CURRENT_HINT}><NumberField path="/provision/settings/current_ma" /></span>
    <span className="board-value-field" title={RECOVERY_HINT}><Toggle path="/provision/settings/recovery_enabled" /><span className="label-caps board-value-caption" aria-hidden="true">recovery</span></span>
  </BoardValueCard>;
}

/**
 * A labelled switch in the action strip. The text toggles too, as a form label would — but a real
 * `<label>` cannot be used: `ResettableControl` renders its reset button before the switch, so a
 * label would activate the reset instead of the switch.
 */
function StripSwitch({ label, hint, disabled, onToggle, children }: { label: ReactNode; hint: string; disabled?: boolean; onToggle: () => void; children: ReactNode }) {
  return <span className={`strip-switch${disabled ? ' is-disabled' : ''}`} title={hint} aria-disabled={disabled || undefined}>
    <span className="strip-switch-label" onClick={disabled ? undefined : onToggle}>{label}</span>
    {children}
  </span>;
}

const AUTO_FLASH_HINT = 'Arm the fixture: every board inserted is flashed and provisioned without pressing anything. While armed, the manual flash button is disabled.';
const FORCE_HINT = 'Auto-flash only: program even when the board already carries every image this pass would write. A manual flash always programs.';
const SOUND_HINT = 'Bench sounds: a board connecting or dropping, a run starting, pass, fail and abort.';

function FlashActionStrip({ soundEnabled, onSoundEnabledChange }: { soundEnabled: boolean; onSoundEnabledChange: (enabled: boolean) => void }) {
  const probe = useBool('/probe/connected');
  const target = useBool('/probe/target_present');
  const flashBusy = useBool('/flash/busy');
  const runBusy = useBool('/run/busy');
  const armed = useBool('/flash/armed');
  const autoEnabled = useParam<boolean>('/flash/auto_enabled');
  const forceWrite = useParam<boolean>('/flash/force_write');
  const flashStep = useText('/flash/step');
  const flashPhase = useText('/flash/phase');
  const flashProgress = useNumber('/flash/progress');
  const runStep = useText('/run/step_name');
  const runPhase = useEnumName('/run/phase');
  const runProgress = useNumber('/run/step_fraction');
  const busy = flashBusy || runBusy;
  const progress = Math.max(0, Math.min(1, flashBusy ? flashProgress : runProgress));
  const detail = (flashBusy ? flashStep || flashPhase : runStep || runPhase) || 'Starting';
  const state = busy ? 'WORKING' : target ? 'READY' : probe ? 'WAITING FOR MCU' : 'WAITING FOR ST-LINK';
  // Force belongs to auto-flash alone, and must not reach over and enable it: arming the rig is
  // what disables "Flash / Provision now", so a toggle meant to make flashing more thorough used
  // to take the manual button away. Manual passes program unconditionally and ignore this.
  const toggleForceWrite = () => forceWrite.set(!forceWrite.value);
  return <section className={`action-strip ${busy ? 'is-busy' : target ? 'is-ready' : 'is-waiting'}`} data-av-surface="test-runner">
    {busy && <span className="action-progress" style={{ width: `${Math.round(progress * 100)}%` }} />}
    <div className="action-state"><strong>{state}</strong>{busy && <span>{detail} · {Math.round(progress * 100)}%</span>}</div>
    <div className="board-values"><SerialNumberCard /><ModuleSettingsCard /></div>
    <ManualFlashButton />
    <div className={`auto-flash${armed ? ' is-armed' : ''}`}>
      <StripSwitch label={<><strong>Auto flash</strong>{armed && <small>Armed</small>}</>} hint={AUTO_FLASH_HINT} onToggle={() => autoEnabled.set(!autoEnabled.value)}><Toggle path="/flash/auto_enabled" /></StripSwitch>
      <StripSwitch label="Force" hint={FORCE_HINT} disabled={!forceWrite.decl || flashBusy} onToggle={toggleForceWrite}><Toggle path="/flash/force_write" /></StripSwitch>
      <StripSwitch label="Sound" hint={SOUND_HINT} onToggle={() => onSoundEnabledChange(!soundEnabled)}><button type="button" className={`toggle${soundEnabled ? ' is-on' : ''}`} role="switch" aria-checked={soundEnabled} aria-label={soundEnabled ? 'Disable bench sounds' : 'Enable bench sounds'} onClick={() => onSoundEnabledChange(!soundEnabled)}><span className="toggle-knob" /></button></StripSwitch>
    </div>
  </section>;
}

interface AssociationRow { serial: number; uid: string; status: string; active: boolean; created_ms: number; updated_ms: number }

interface SerialCandidateRow { source: string; serial: number; blocked_by: string | null; needs_override: boolean; action: string; detail: string }

interface DeviceHistoryRow { uid: string; first_seen_ms: number; last_seen_ms: number; last_seen_serial: number | null; reads: number }

interface ProvisionDoc {
  candidates: SerialCandidateRow[];
  suggested_serial: number | null;
  next_free_serial: number;
  on_board_serial: number | null;
  database_serial: number | null;
  database_status: string;
  entered_serial_holder: string | null;
  device_history: DeviceHistoryRow | null;
  registry: AssociationRow[];
}

/**
 * One poll of `/api/bench/provision`, shared by every reader.
 *
 * The serial strip at the top and the chooser at the bottom want the same document, and two
 * independent intervals would double the request rate and let the two halves of one answer
 * disagree for up to a second — which is exactly the kind of flicker that makes an operator
 * distrust a number. Module-level rather than context because the two live in different subtrees
 * of the page and threading a provider between them buys nothing.
 */
let provisionDoc: ProvisionDoc | null = null;
const provisionReaders = new Set<(doc: ProvisionDoc | null) => void>();
let provisionTimer: number | undefined;

function useProvisionDoc(): ProvisionDoc | null {
  const [doc, setDoc] = useState<ProvisionDoc | null>(provisionDoc);
  useEffect(() => {
    provisionReaders.add(setDoc);
    if (provisionTimer === undefined) {
      const load = async () => {
        try {
          const response = await fetch('/api/bench/provision', { cache: 'no-store' });
          if (!response.ok) return;
          provisionDoc = await response.json();
          for (const reader of provisionReaders) reader(provisionDoc);
        } catch { /* the host may be restarting */ }
      };
      void load();
      provisionTimer = window.setInterval(load, 1000);
    }
    return () => {
      provisionReaders.delete(setDoc);
      if (provisionReaders.size === 0 && provisionTimer !== undefined) {
        window.clearInterval(provisionTimer);
        provisionTimer = undefined;
      }
    };
  }, []);
  return doc;
}

/** `1` → "1", `null`/`0` → an em dash, so an absent number reads as absent rather than as zero. */
function serialText(serial: number | null | undefined): string {
  return serial && serial > 0 ? String(serial) : '—';
}

/**
 * The three places a serial can come from, at the top of the page, one press each.
 *
 * This exists because the answer to "what number should this board have?" was three scrolls below
 * the fold. The strip is where an operator already looks — it carries READY and the flash button —
 * so the sources belong here, as small as they can be while still being readable and clickable.
 *
 * DATABASE is shown even when empty, and that is the whole point of the element. A board whose
 * identity page was erased looks identical to a new one until something says the registry knows
 * it; leaving the chip out when there is no number would hide exactly the case it was added for.
 */
function SerialSources() {
  const doc = useProvisionDoc();
  const boardPresent = useBool('/probe/target_present');
  if (!doc || !boardPresent) return null;
  const byAction = (name: string) => doc.candidates.find((candidate) => candidate.action === name);
  return <div className="serial-sources" role="group" aria-label="Serial sources">
    <SerialSourceChip label="board" serial={doc.on_board_serial} doc={doc}
      candidate={byAction('take_onboard_serial')} action="take_onboard_serial"
      empty="No serial on this board's identity page." />
    <SerialSourceChip label="database" serial={doc.database_serial} doc={doc}
      candidate={byAction('use_database_serial')} action="use_database_serial"
      empty={databaseEmptyReason(doc)} />
    <SerialSourceChip label="free" serial={doc.next_free_serial} doc={doc}
      candidate={byAction('take_next_free_serial')} action="take_next_free_serial" empty="" />
  </div>;
}

/** One source chip. Its own component so the action counter can be a hook, not a callback. */
function SerialSourceChip({ label, serial, doc, candidate, action, empty }: { label: string; serial: number | null; doc: ProvisionDoc; candidate?: SerialCandidateRow; action: string; empty: string }) {
  const counter = useParam<number>(`/actions/${action}`);
  const entered = useNumber('/provision/serial_to_provision');
  const has = !!serial && serial > 0;
  const blocked = !!candidate?.blocked_by;
  const current = has && serial === entered;
  const suggested = has && serial === doc.suggested_serial;
  const seen = !has && !!doc.device_history;
  const title = has
    ? `${candidate?.detail ?? ''}${blocked ? ` Held by ${candidate?.blocked_by}.` : ''} Click to use serial ${serial}.`
    : empty;
  // The title lives on a wrapper, not on the button: a disabled button does not raise hover
  // events in WebKit, so the one explanation an empty chip exists to give would never be read.
  return <span className="serial-source-slot" title={title}>
    <button
      type="button"
      className={`serial-source${current ? ' is-current' : ''}${blocked ? ' is-blocked' : ''}${suggested ? ' is-suggested' : ''}${seen ? ' is-seen' : ''}`}
      aria-label={`${label}: ${serialText(serial)}. ${title}`}
      disabled={!candidate || !counter.decl}
      onClick={() => counter.set((counter.value ?? 0) + 1)}
    >
      <span className="label-caps">{label}</span>
      <span className="serial-source-value">{serialText(serial)}</span>
    </button>
  </span>;
}

/**
 * Why the database has no number for this MCU — which is two different situations.
 *
 * "Never seen" and "seen repeatedly but never registered" look the same in the registry and mean
 * opposite things. The second is a board whose reservation kept failing, and telling an operator
 * it is new is how it gets issued a second serial.
 */
function databaseEmptyReason(doc: ProvisionDoc): string {
  const history = doc.device_history;
  if (!history) return 'This MCU is new to this bench — no record of it at all.';
  const when = new Date(history.last_seen_ms).toLocaleString();
  const claimed = history.last_seen_serial
    ? ` It was last read carrying serial ${history.last_seen_serial}, but that was never recorded against it — check whether something else holds that number.`
    : '';
  return `Seen before (${history.reads} reads, last ${when}) but never registered.${claimed}`;
}

const CANDIDATE_TITLE: Record<string, string> = {
  registry: 'In the registry',
  'on-board': 'On the board',
  'next-free': 'Next free',
};

const PREFER_REGISTRY_HINT = 'Lock: on every board read, take the serial the registry already holds for that MCU instead of the one written on its identity page. For re-flashing boards that are already registered. Leave it off while first provisioning, when the board is the only source there is.';

/**
 * Which serial this board should get, and one press to give it that.
 *
 * The panel this replaces stated the problem twice — a badge reading "conflict" and a sentence
 * naming the other MCU — and then left the operator to work out the answer in a number box. The
 * question it never answered is the only one being asked: *which number should this board have?*
 *
 * So the candidates are the interface. Each is a real number with its provenance, its consequence
 * and its cost, and taking one is a single press that also arms the override when — and only
 * when — that number renumbers something. The worker computes them (`serial_candidates`), so what
 * is offered here and what is enforced at reservation cannot drift apart.
 */
function SerialChooser({ uid }: { uid: string }) {
  const snapshot = useProvisionDoc();
  const entered = useNumber('/provision/serial_to_provision');
  const candidates = snapshot?.candidates ?? [];
  const holder = snapshot?.entered_serial_holder ?? '';
  // The blocking row carries the status and dates the bare UID cannot. Worth the lookup: "held by
  // a board provisioned months ago" and "held by this morning's simulator run" are the same
  // sentence with very different answers.
  const holderRow = holder ? snapshot?.registry.find((row) => row.active && row.uid === holder) : undefined;
  if (!uid) return <div className="serial-chooser is-idle"><small>Connect and read a board to choose its serial.</small></div>;
  return <div className={`serial-chooser${holder ? ' is-conflict' : ''}`}>
    <header>
      <div>
        <strong>Serial for this board</strong>
        <small>MCU <code>{uid}</code></small>
      </div>
      <PreferRegistryLock />
    </header>
    {holder && <div className="serial-clash" role="alert">
      <div className="serial-clash-head"><Badge tone="error">conflict</Badge><strong>Serial {entered} already belongs to a different MCU.</strong></div>
      <div className="serial-clash-holder">
        <code>{holder}</code>
        <small>{holderRow ? `${holderRow.status} · last updated ${new Date(holderRow.updated_ms).toLocaleString()}` : 'held in the registry'}</small>
      </div>
      <small className="serial-clash-note">Pick one of the numbers below. Only the number you pick is written; nothing is changed until you flash.</small>
    </div>}
    <div className="candidate-row" role="group" aria-label="Serial candidates">
      {candidates.length === 0 ? <small>No serial can be offered yet — the provisioning database is unavailable.</small> : candidates.map((candidate) => {
        const suggested = candidate.serial === snapshot?.suggested_serial;
        const chosen = candidate.serial === entered;
        return <div key={candidate.source} className={`candidate${suggested ? ' is-suggested' : ''}${candidate.blocked_by ? ' is-blocked' : ''}${chosen ? ' is-chosen' : ''}`}>
          <div className="candidate-head">
            <span className="candidate-serial">{candidate.serial}</span>
            <div className="candidate-tags">
              <span className="label-caps">{CANDIDATE_TITLE[candidate.source] ?? candidate.source}</span>
              {suggested && <Badge tone="ok">suggested</Badge>}
              {chosen && !suggested && <Badge tone="idle">in the box</Badge>}
            </div>
          </div>
          <small className="candidate-detail">{candidate.detail}</small>
          {candidate.blocked_by && <small className="candidate-blocked">held by <code>{candidate.blocked_by}</code></small>}
          <Action
            path={`/actions/${candidate.action}`}
            variant={candidate.blocked_by ? 'danger' : suggested ? 'primary' : 'default'}
            className="candidate-take"
          >{candidate.blocked_by ? `Move ${candidate.serial} here` : `Take ${candidate.serial}`}</Action>
        </div>;
      })}
    </div>
  </div>;
}

function PreferRegistryLock() {
  const p = useParam<boolean>('/provision/prefer_registry_serial');
  const on = !!p.value;
  return <label className={`prefer-registry${on ? ' is-on' : ''}`} title={PREFER_REGISTRY_HINT}>
    <Toggle path="/provision/prefer_registry_serial" />
    <span>Always take the registry serial</span>
    <svg viewBox="0 0 16 16" width="13" height="13" aria-hidden="true"><rect x="3" y="7.5" width="10" height="6.5" rx="1.5" fill="currentColor" /><path d={on ? 'M5 7.5V5a3 3 0 0 1 6 0v2.5' : 'M5 7.5V5a3 3 0 0 1 6 0'} fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" /></svg>
    {!p.decl && <small>unavailable</small>}
  </label>;
}

function ProvisionPanel() {
  const dbOk = useBool('/provision/database_ok');
  const dbError = useText('/provision/database_error');
  const identity = useText('/provision/identity_state');
  const existing = useNumber('/provision/on_board_serial');
  const pending = useBool('/provision/pending_replug');
  const reservation = useText('/provision/reservation');
  const source = useText('/provision/settings/source');
  const dbSerial = useNumber('/provision/database_serial');
  const dbStatus = useText('/provision/database_status');
  const mcuUid = useText('/mcu/uid');
  const serial = serialState(useSerialView());
  const settingsView = useSettingsView();
  const settings = settingsState(settingsView);
  const settingsWhy = !settingsView.boardPresent ? 'no MCU is answering' : null;
  const readWhy = settingsView.locked ? 'Module Settings are locked; unlock them to read into them' : settingsWhy;
  return <section className="provision-panel" aria-label="Board provisioning">
    <header><div><strong>Board provisioning</strong><small>Serial identity is durable and separate from the 1–127 RS485 address.</small></div><Badge tone={serial.tone}>{serial.word}</Badge></header>
    {!dbOk && <Banner tone="error">Provisioning is blocked: {dbError || 'the local database is unavailable'}. Diagnostics and tests remain usable.</Banner>}
    {pending && <Banner tone="warn">Remove and reconnect this board. Its pending UID, serial, and firmware will be verified without another flash.</Banner>}
    <SerialChooser uid={mcuUid} />
    <div className="provision-grid">
      <section className="provision-fields"><header><strong>Serial for this board</strong><small>Where each offered number comes from. The Database tab owns the library and next-serial floor.</small></header><Fact label="On-board serial number" value={existing > 0 ? String(existing) : 'none'} /><Fact label="Registry serial for this MCU" value={dbSerial > 0 ? `${dbSerial}${dbStatus ? ` · ${dbStatus}` : ''}` : 'not in registry'} tone={dbSerial > 0 && existing > 0 && dbSerial !== existing ? 'warn' : undefined} /><Fact label="Identity status" value={identity || 'unknown'} tone={identity === 'corrupt' || identity === 'foreign-uid' ? 'error' : undefined} /><Fact label="Reservation" value={reservation || 'none'} /><div className="button-row"><Action path="/actions/keep_onboard_serial" why={existing <= 0 ? 'no valid on-board serial' : null} variant="quiet">Keep on-board serial</Action><Action path="/actions/use_pcb_serial" variant="danger">Override with typed serial</Action></div></section>
      <section className="provision-fields"><header><strong>Module settings</strong><small>Edit them in the Module Settings card above; they are written with the serial on the next flash. Read from board discards your edits; the padlock on the card keeps them across boards.</small></header><Fact label="On board" value={settings.detail.replace(/^board: /, '')} /><Fact label="Entered" value={settingsSummary(settingsView.currentMa, settingsView.recovery)} tone={settings.changed ? 'warn' : undefined} /><Fact label="Settings source" value={source === 'flash' ? 'settings journal' : 'firmware defaults'} /><div className="button-row"><Action path="/actions/read_settings" why={readWhy}>Read from board</Action><Action path="/actions/write_settings" variant="primary" why={settingsWhy}>Write to board</Action></div></section>
    </div>
  </section>;
}

function FlashDeviceActions() {
  const bootState = useText('/flash/boot_state');
  const needsReplug = useBool('/flash/needs_replug');
  const probe = useBool('/probe/connected');
  const target = useBool('/probe/target_present');
  const busy = useBool('/flash/busy');
  const armed = useBool('/flash/armed');
  const runBusy = useBool('/run/busy');
  const route = useEnumName('/motion/route');
  const serialConnected = useBool('/serial/connected');
  const rs485Connected = useBool('/rs485/connected');
  const routeConnected = route === 'rs485' ? rs485Connected : serialConnected;
  const fixtureWhy = busy ? 'a flash pass is running' : runBusy ? 'a test plan is running' : armed ? 'disable auto-flash first' : !probe ? 'no ST-Link' : !target ? 'no MCU is answering' : null;
  const firmwareWhy = busy || armed ? 'flashing owns the fixture' : runBusy ? 'a test plan is running' : !routeConnected ? `${route} is not connected` : null;
  const bootLabel = needsReplug ? 'replug required' : bootState === 'running' || bootState === 'checking' ? bootState : 'check available';
  return <section className="flash-device-actions" aria-label="MCU recovery and device actions">
    <header><div><strong>After-flash and recovery</strong><small>SWD controls work even when VCOM is not available.</small></div><Badge tone={needsReplug ? 'warn' : bootState === 'running' ? 'ok' : 'idle'}>{bootLabel}</Badge></header>
    {needsReplug && <Banner tone="warn">Flash verified, but this virgin board needs power removed. Unplug and replug it, then press Check boot.</Banner>}
    {!needsReplug && bootState === 'checking' && <Banner tone="info">Checking application execution…</Banner>}
    <div className="flash-action-groups">
      <div><strong>Hardware control</strong><small>Release the MCU from the debugger or inspect its boot state.</small><span className="button-row"><Action path="/actions/reset_mcu" variant="primary" why={fixtureWhy}>Reset &amp; start (SWD)</Action><Action path="/actions/check_boot" why={fixtureWhy}>Check boot</Action></span></div>
      <div><strong>Firmware control</strong><small>Ask the running firmware to reboot over the Test tab&apos;s {route.toUpperCase()} route.</small><span className="button-row"><Action path="/actions/reboot" why={firmwareWhy}>Reboot over {route.toUpperCase()}</Action></span></div>
      <div><strong>Inspection</strong><small>Refresh target identity or rescan fixture hardware and images.</small><span className="button-row"><Action path="/actions/read_device" why={busy ? 'a flash pass is running' : null}>Read device identity</Action><Action path="/actions/rescan_firmware" why={busy || armed || runBusy ? 'the fixture is busy' : null}>Rescan all</Action></span></div>
    </div>
  </section>;
}

function FlashTab() {
  const simPresent = useParam<boolean>('/sim/module_present');
  return <Panel title="Flash">
    <div className="flash-operations" data-av-surface="firmware">
      <SetupPicker />
      <ProvisionPanel />
      {simPresent.decl && <div className="simulation-fixture"><span>Simulated board in fixture</span><Toggle path="/sim/module_present" /></div>}
      <FlashDeviceActions />
    </div>
  </Panel>;
}

function LinkState({ connected, observed, detail }: { connected: boolean; observed: string; detail: string }) {
  return <div className="link-state"><Badge tone={connected ? 'ok' : 'offline'}>{connected ? observed : 'down'}</Badge><span>{detail || (connected ? 'receiving evidence' : 'not connected')}</span></div>;
}

interface ProcedureEntry {
  name: string;
  ok: boolean;
  kind?: string;
  requires?: { firmware?: string | null; transport?: string | null };
  steps?: number;
  criteria?: number;
  destructive?: boolean;
  error?: string;
}

function ProcedureRunner() {
  const selected = useParam<string>('/plan/selected');
  const run = useParam<number>('/actions/run');
  const route = useEnumName('/motion/route');
  const serialKind = useEnumName('/serial/observed');
  const rs485Kind = useEnumName('/rs485/observed');
  const serialConnected = useBool('/serial/connected');
  const rs485Connected = useBool('/rs485/connected');
  const busy = useBool('/run/busy');
  const flashBusy = useBool('/flash/busy');
  const flashArmed = useBool('/flash/armed');
  const flashLocked = flashBusy || flashArmed;
  const runningPlan = useText('/run/plan');
  const phase = useEnumName('/run/phase');
  const step = useText('/run/step_name');
  const stepIndex = useNumber('/run/step_index');
  const stepCount = useNumber('/run/step_count');
  const fraction = useNumber('/run/step_fraction');
  const elapsed = useNumber('/run/elapsed_s');
  const fieldDetail = useText('/field_update/detail');
  const [plans, setPlans] = useState<ProcedureEntry[]>([]);
  const [directory, setDirectory] = useState('');
  const [confirmPlan, setConfirmPlan] = useState('');
  useEffect(() => {
    let cancelled = false;
    void fetch('/api/bench/plans', { cache: 'no-store' })
      .then((response) => response.ok ? response.json() : Promise.reject())
      .then((body) => {
        if (!cancelled) {
          setPlans(body.plans ?? []);
          setDirectory(body.dir ?? '');
        }
      })
      .catch(() => {});
    return () => { cancelled = true; };
  }, []);
  const connected = route === 'rs485' ? rs485Connected : serialConnected;
  const observed = route === 'rs485' ? rs485Kind : serialKind;
  const incompatibility = (plan: ProcedureEntry): string | null => {
    if (!plan.ok) return plan.error || 'procedure does not parse';
    if (!connected) return `${route} is not connected`;
    const required = plan.requires?.transport;
    if (required === 'vcp' && observed !== 'vcp') return 'requires a production VCOM link';
    // `sim` counts: it is the same `Rs485Link` over `SimBus`, and `TransportRequirement::accepts`
    // says the same thing on the Rust side. The run is stamped `"transport": "sim"` in the
    // report so it can never be read later as a statement about hardware.
    if (required === 'rs485' && !observed.startsWith('rs485') && observed !== 'sim') return 'requires an RS485 link';
    if (required === 'bench-ascii' && observed !== 'bench-ascii') return 'requires the bench serial protocol';
    if (busy) return `${runningPlan || 'another procedure'} is running`;
    if (flashLocked) return 'flashing owns the fixture';
    return null;
  };
  const start = (plan: ProcedureEntry) => {
    if (plan.destructive && confirmPlan !== plan.name) {
      setConfirmPlan(plan.name);
      window.setTimeout(() => setConfirmPlan((value) => value === plan.name ? '' : value), 5000);
      return;
    }
    setConfirmPlan('');
    selected.set(plan.name);
    window.setTimeout(() => run.set((run.value ?? 0) + 1), 0);
  };
  return <Panel title="Procedures" right={<Badge tone={busy ? 'active' : 'idle'}>{busy ? 'running' : `${plans.length} available`}</Badge>}>
    {busy && <div className="procedure-progress">
      <div><strong>{runningPlan}</strong><span>{phase} - {step || 'starting'} - {elapsed}s</span>{step === 'Bootloader upload check' && fieldDetail && <small>{fieldDetail}</small>}</div>
      <div className="procedure-progress-track"><i style={{ width: `${Math.round(fraction * 100)}%` }} /></div>
      <span>Step {Math.min(stepIndex + 1, stepCount || 1)} of {stepCount || 1}</span>
      <Action path="/actions/abort" variant="danger">Abort procedure</Action>
    </div>}
    {plans.length === 0 ? <EmptyState inline detail="No procedure files were found." /> :
      <div className="procedure-list" role="list" aria-label="Test procedures">{plans.map((plan) => {
        const why = incompatibility(plan);
        const active = busy && runningPlan === plan.name;
        return <article key={plan.name} className={`procedure-row${active ? ' is-running' : ''}`} role="listitem">
          <div><strong>{plan.name}</strong><small>{plan.kind || 'invalid'} - {plan.steps ?? 0} steps - {plan.criteria ?? 0} criteria</small></div>
          <span className="procedure-tags"><Badge tone="idle">{plan.requires?.transport || 'either route'}</Badge>{plan.requires?.firmware && <Badge tone="idle">{plan.requires.firmware}</Badge>}{plan.destructive && <Badge tone="warn">rewrites flash</Badge>}</span>
          <span title={why ?? (plan.destructive ? 'Press twice within five seconds' : `Run ${plan.name}`)}><Button variant={confirmPlan === plan.name ? 'danger' : active ? 'quiet' : 'primary'} disabled={!!why} onClick={() => start(plan)}>{active ? 'Running' : confirmPlan === plan.name ? 'Confirm rewrite' : 'Run'}</Button></span>
          {plan.error && <small className="procedure-error">{plan.error}</small>}
        </article>;
      })}</div>}
    <footer className="procedure-directory" title={directory}>{directory}</footer>
  </Panel>;
}

function QuickCommands() {
  const route = useEnumName('/motion/route');
  const serialKind = useEnumName('/serial/observed');
  const rs485Kind = useEnumName('/rs485/observed');
  const serialConnected = useBool('/serial/connected');
  const rs485Connected = useBool('/rs485/connected');
  const connected = route === 'rs485' ? rs485Connected : serialConnected;
  const runBusy = useBool('/run/busy');
  const flashBusy = useBool('/flash/busy');
  const flashArmed = useBool('/flash/armed');
  const flashLocked = flashBusy || flashArmed;
  const kind = route === 'rs485' ? rs485Kind : serialKind;
  const rs485 = kind.startsWith('rs485') || kind === 'sim';
  // Not `|| kind === 'sim'`: the threshold census is a VCOM-only routine that the simulator
  // answers with a bare ACK and no samples, so enabling it there would be a button that lies.
  const vcom = kind === 'vcp';
  const locked = runBusy || flashLocked;
  const why = (supported = true) => !connected ? `${route} is not connected` : locked ? 'the fixture is busy' : !supported ? `${kind} cannot express this command` : null;
  return <Panel title="Quick routines" right={<Badge tone={connected ? 'ok' : 'offline'}>{route} / {connected ? kind : 'down'}</Badge>}>
    <div className="quick-command-groups">
      <section><h3>Module</h3><div className="button-row">
        <Action path="/actions/identify" why={why()}>Identify</Action><Action path="/actions/poll" why={why()}>Poll status</Action>
        <Action path="/actions/routine_startup" why={why(vcom || rs485)}>Startup</Action><Action path="/actions/calibrate_module" why={why(vcom || rs485)}>Calibrate both axes</Action>
        <Action path="/actions/reboot" why={why(vcom || rs485)}>Reboot</Action><Action path="/actions/escape" why={!connected ? `${route} is not connected` : null} variant="danger">Escape</Action>
      </div></section>
      <section><h3>Axis diagnostics</h3><div className="button-row">
        <Action path="/actions/home_a" why={why(vcom || rs485)}>Home A</Action><Action path="/actions/home_b" why={why(vcom || rs485)}>Home B</Action>
        <Action path="/actions/backlash_a" why={why(rs485)}>Backlash A</Action><Action path="/actions/backlash_b" why={why(rs485)}>Backlash B</Action>
        <Action path="/actions/unjam_a" why={why(rs485)}>Unjam both axes</Action>
      </div></section>
      <section className="quick-fields"><h3>Driver and optical</h3>
        <div><Row label="Current (A)"><NumberField path="/test/current_a" /></Row><Action path="/actions/set_current" why={why(rs485)}>Apply current</Action></div>
        <div><Row label="Microstep"><NumberField path="/test/microstep" /></Row><Action path="/actions/set_microstep" why={why(rs485)}>Apply microstep</Action></div>
        <div><Row label="Threshold"><NumberField path="/test/home_threshold" /></Row><Action path="/actions/set_threshold" why={why(vcom || rs485)}>Apply threshold</Action></div>
        <div><Row label="Census speed"><NumberField path="/test/census_speed" /></Row><span className="button-row"><Action path="/actions/census_a" why={why(vcom)}>Census A</Action><Action path="/actions/census_b" why={why(vcom)}>Census B</Action></span></div>
      </section>
    </div>
  </Panel>;
}

type HomeEvidence = {
  seq: number; atMs: number; axis: 'A' | 'B' | 'module'; kind: string;
  outcome: 'ok' | 'fail' | 'measure'; detail: string;
  threshold?: number; width?: number; midpoint?: number; backlash?: number; cycleSteps?: number;
};

function parseHomeEvidence(line: { seq?: number; at_ms?: number; message?: string }): HomeEvidence | null {
  const detail = line.message ?? '';
  const axis = detail.includes('MotionControl_A') ? 'A' : detail.includes('MotionControl_B') ? 'B' : 'module';
  let match = detail.match(/Cycle = (\d+) full steps \(expected (\d+)\)/);
  if (match) return { seq: line.seq ?? 0, atMs: line.at_ms ?? 0, axis, kind: 'cycle', outcome: 'measure', detail, cycleSteps: Number(match[1]) };
  match = detail.match(/pass (\d+): lead=(-?\d+) trail=(-?\d+) w=(\d+) mid=(-?\d+)/);
  if (match) return { seq: line.seq ?? 0, atMs: line.at_ms ?? 0, axis, kind: `edge pass ${match[1]}`, outcome: 'measure', detail, width: Number(match[4]), midpoint: Number(match[5]) };
  match = detail.match(/fastHome OK: datum=(-?\d+) w=(\d+) backlash=(-?\d+) T=(\d+)/);
  if (match) return { seq: line.seq ?? 0, atMs: line.at_ms ?? 0, axis, kind: 'home', outcome: 'ok', detail, midpoint: Number(match[1]), width: Number(match[2]), backlash: Number(match[3]), threshold: Number(match[4]) };
  match = detail.match(/fastHome result: status=fail class=([^ ]+) reason=(.*)$/);
  if (match) return { seq: line.seq ?? 0, atMs: line.at_ms ?? 0, axis, kind: match[1], outcome: 'fail', detail: match[2] };
  if (detail.includes('survey recovery')) return { seq: line.seq ?? 0, atMs: line.at_ms ?? 0, axis, kind: 'speed survey', outcome: detail.includes(' OK:') ? 'ok' : 'measure', detail };
  return null;
}

function downloadEvidence(name: string, body: string, type: string) {
  const url = URL.createObjectURL(new Blob([body], { type }));
  const anchor = document.createElement('a'); anchor.href = url; anchor.download = name; anchor.click();
  URL.revokeObjectURL(url);
}

function HomingDiagnostics() {
  const [events, setEvents] = useState<HomeEvidence[]>([]);
  useEffect(() => {
    let cursor = 0; let stopped = false;
    const poll = async () => {
      try {
        const response = await fetch(`/api/bench/log?from=${cursor}`, { cache: 'no-store' });
        if (response.ok) {
          const payload = await response.json(); cursor = payload.next ?? cursor;
          const parsed = (payload.lines ?? []).map(parseHomeEvidence).filter(Boolean) as HomeEvidence[];
          if (parsed.length) setEvents((current) => [...current, ...parsed].slice(-500));
        }
      } catch { /* host restart: next poll retries */ }
      if (!stopped) window.setTimeout(poll, 750);
    };
    void poll(); return () => { stopped = true; };
  }, []);
  const homes = events.filter((event) => event.kind === 'home');
  const failures = events.filter((event) => event.outcome === 'fail');
  const latest = (axis: 'A' | 'B') => [...homes].reverse().find((event) => event.axis === axis);
  const exportJson = () => downloadEvidence(`portal-homing-${Date.now()}.json`, JSON.stringify(events, null, 2), 'application/json');
  const exportCsv = () => {
    const quote = (value: unknown) => `"${String(value ?? '').replaceAll('"', '""')}"`;
    const rows = events.map((event) => [event.seq, event.atMs, event.axis, event.kind, event.outcome, event.threshold, event.width, event.midpoint, event.backlash, event.cycleSteps, event.detail].map(quote).join(','));
    downloadEvidence(`portal-homing-${Date.now()}.csv`, ['seq,at_ms,axis,kind,outcome,threshold,width_usteps,midpoint_usteps,backlash_usteps,cycle_full_steps,detail', ...rows].join('\n'), 'text/csv');
  };
  return <Panel title="Homing diagnostics" right={<Badge tone={failures.length ? 'warn' : homes.length ? 'ok' : 'idle'}>{failures.length ? `${failures.length} classified failure(s)` : homes.length ? 'repeatable evidence captured' : 'waiting for a home'}</Badge>}>
    <div className="homing-summary">
      {(['A', 'B'] as const).map((axis) => { const value = latest(axis); return <section key={axis}><strong>Axis {axis}</strong><Fact label="Result" value={value?.outcome ?? '—'} tone={value?.outcome === 'fail' ? 'error' : undefined} /><Fact label="Threshold" value={value?.threshold ?? '—'} /><Fact label="Feature width" value={value?.width == null ? '—' : `${value.width} µsteps`} /><Fact label="Datum / backlash" value={value?.midpoint == null ? '—' : `${value.midpoint} / ${value.backlash ?? '—'} µsteps`} /></section>; })}
    </div>
    <div className="homing-evidence-tools"><span>Structured from firmware cycle, two-edge pass, survey, and classified result lines.</span><Button variant="quiet" onClick={() => setEvents([])}>Clear view</Button><Button variant="quiet" disabled={!events.length} onClick={exportJson}>Export JSON</Button><Button variant="quiet" disabled={!events.length} onClick={exportCsv}>Export CSV</Button></div>
    <div className="homing-evidence-table" role="table" aria-label="Homing evidence">
      <header role="row"><span>Axis</span><span>Evidence</span><span>Result</span><span>Width / midpoint</span><span>Detail</span></header>
      {[...events].reverse().slice(0, 40).map((event) => <div role="row" key={`${event.seq}-${event.kind}`} data-outcome={event.outcome}><b>{event.axis}</b><span>{event.kind}</span><span>{event.outcome}</span><span>{event.width ?? '—'} / {event.midpoint ?? '—'}</span><small title={event.detail}>{event.detail}</small></div>)}
      {!events.length && <EmptyState inline detail="Run Startup, Calibrate, Home, or a threshold census to populate this evidence view." />}
    </div>
  </Panel>;
}

function RawSignalPanel() {
  const route = useEnumName('/motion/route');
  const serialConnected = useBool('/serial/connected');
  const rs485Connected = useBool('/rs485/connected');
  const runBusy = useBool('/run/busy');
  const flashBusy = useBool('/flash/busy');
  const flashArmed = useBool('/flash/armed');
  const flashLocked = flashBusy || flashArmed;
  const vcomText = useText('/test/raw/vcom_text');
  const rs485Json = useText('/test/raw/rs485_json');
  const rs485Target = useNumber('/rs485/target');
  const connected = route === 'rs485' ? rs485Connected : serialConnected;
  let invalidJson = false;
  if (route === 'rs485') {
    try { invalidJson = typeof JSON.parse(rs485Json) !== 'object' || Array.isArray(JSON.parse(rs485Json)) || JSON.parse(rs485Json) === null; }
    catch { invalidJson = true; }
  }
  const reason = !connected ? `${route} is not connected` : runBusy || flashLocked ? 'the fixture is busy' : route === 'serial' && !vcomText ? 'enter a VCOM payload' : invalidJson ? 'enter a JSON object' : null;
  return <details className="raw-signal-panel">
    <summary><span>Advanced raw signal</span><Badge tone="warn">single-click send</Badge></summary>
    <Banner tone="warn">This bypasses the command vocabulary, but still uses the selected link's normal framing and address.</Banner>
    {route === 'serial' ? <div className="raw-fields"><Row label="VCOM text"><TextField path="/test/raw/vcom_text" /></Row><Row label="Line ending"><EnumSelect path="/test/raw/line_ending" /></Row></div> :
      <div className="raw-fields"><Row label="MessagePack body (JSON)"><TextField path="/test/raw/rs485_json" /></Row><Fact label="Target" value={rs485Target} /></div>}
    <Action path="/actions/send_raw" variant="danger" why={reason}>Send raw over {route}</Action>
  </details>;
}

/** The gateways a Reworld install ships with. Typing them from memory is not a skill. */
const GATEWAY_PRESETS = ['192.168.1.201:4196', '192.168.1.202:4196'];

const SERIAL_LABELS = { none: 'Choose protocol', vcp: 'Production serial console', 'bench-ascii': 'Bench firmware console' };
const RS485_LABELS = { none: 'Choose transport', 'rs485-serial': 'USB / serial adapter', 'rs485-tcp': 'Ethernet gateway', sim: 'Simulated module (no hardware)' };

/**
 * Where the link is, chosen rather than typed.
 *
 * The bench has always known the answer — `bench_core::survey()` reports every port with its
 * USB product string and serial number, and `/api/bench/ports` has always served it — but the
 * page asked the operator to type `COM15` or `/dev/tty.usbserial-A9K3` from memory. Two escape
 * hatches survive, because the list is not always the answer: an Ethernet gateway is a
 * `host:port` and not a device at all, and a port the OS declines to enumerate (a `socat` pty,
 * for one) still has to be reachable.
 */
function EndpointPicker({ path, transport, probeSerial }: { path: string; transport: string; probeSerial: string }) {
  const endpoint = useParam<string>(path);
  const { ports, loading, refresh } = usePortSurvey();
  const [manual, setManual] = useState(false);
  const value = endpoint.value ?? '';
  const gateway = transport === 'rs485-tcp';
  const typing = manual || (!!value && !ports.some((port) => port.name === value));
  const label = gateway ? 'Gateway address' : 'Port';
  return <div className="endpoint-picker">
    <header>
      <span className="label-caps">{label}</span>
      <span className="endpoint-tools">
        {!gateway && <button type="button" className="text-button" onClick={() => setManual(!manual)}>{typing ? 'choose from list' : 'type manually'}</button>}
        {!gateway && <button type="button" className="text-button" disabled={loading} onClick={() => void refresh()}>{loading ? 'scanning…' : 'refresh'}</button>}
      </span>
    </header>
    {gateway ? <>
      <TextField path={path} />
      <div className="button-row">{GATEWAY_PRESETS.map((preset) =>
        <Button key={preset} variant="quiet" disabled={!endpoint.decl} onClick={() => endpoint.set(preset)}>{preset}</Button>
      )}</div>
    </> : typing ? <TextField path={path} /> :
      ports.length === 0 ? <EmptyState inline detail="No serial ports are attached. Connect the adapter and refresh, or type the device path." /> :
      <div className="choice-list" role="listbox" aria-label="Serial ports">{ports.map((port) =>
        <ChoiceRow key={port.name} selected={value === port.name} title={port.name}
          detail={[port.product, port.serial_number, port.kind].filter(Boolean).join(' · ')}
          // The same USB-serial rule `survey::paired_vcom_port` uses on the Rust side, so this
          // badge and the post-flash auto-attach can never point at different ports.
          badges={probeSerial && port.serial_number?.toLowerCase() === probeSerial.toLowerCase() ? <Badge tone="ok">probe VCOM</Badge> : undefined}
          onClick={() => endpoint.set(value === port.name ? '' : port.name)} />
      )}</div>}
  </div>;
}

/** One line of "and the route you are not looking at is doing this". */
function OtherRoute({ name, connected, observed, endpoint }: { name: string; connected: boolean; observed: string; endpoint: string }) {
  return <footer className="other-route">
    <span>Other route</span>
    <Badge tone={connected ? 'ok' : 'offline'}>{name}</Badge>
    <small>{connected ? `${observed}${endpoint ? ` on ${endpoint}` : ''}` : 'not connected'}</small>
  </footer>;
}

/**
 * The link, first on the page.
 *
 * Every actionable control below this is gated on one expression — is the *selected route*
 * connected — and when it is not, the whole tab is grey. The controls that answer that were
 * previously the fourth section down, under three panels of disabled buttons, which is a tab
 * that looks broken rather than one that is waiting. So the route selector and the connection
 * it selects are now the same control.
 *
 * Only the selected route's controls are drawn. Both lanes can still be open at once — that is
 * a real and useful state, and `Op::Identify` on one says nothing about the other — so the
 * other lane keeps a one-line summary here and its own item in the status bar.
 */
function LinkPanel() {
  const route = useEnumName('/motion/route');
  const rs485 = route === 'rs485';
  // The page detects simulation the way the rest of this file does: by asking whether the
  // `/sim/*` controls were declared at all, never by being told.
  const simulated = !!useParam<boolean>('/sim/module_present').decl;
  const { ports, probes } = usePortSurvey();
  const probeSelected = useText('/probe/selected');
  const probeSerial = probes.find((probe) => probe.identifier === probeSelected)?.serial_number ?? '';

  const serialConnected = useBool('/serial/connected'), rs485Connected = useBool('/rs485/connected');
  const serialObserved = useEnumName('/serial/observed'), rs485Observed = useEnumName('/rs485/observed');
  const serialDesired = useEnumName('/serial/desired'), rs485Desired = useEnumName('/rs485/desired');
  const serialPort = useText('/serial/port'), rs485Endpoint = useText('/rs485/endpoint');
  const serialDetail = useText('/serial/detail'), rs485Detail = useText('/rs485/detail');

  const view: LinkView = rs485
    ? { route, connected: rs485Connected, desired: rs485Desired, endpoint: rs485Endpoint, detail: rs485Detail }
    : { route, connected: serialConnected, desired: serialDesired, endpoint: serialPort, detail: serialDetail };
  const observed = rs485 ? rs485Observed : serialObserved;
  const connectWhy = connectBlocker(view);
  const blocker = linkBlocker(view);
  // `none` has nothing to address and `sim` addresses itself; rendering a port field for either
  // would be a control that cannot be right.
  const addressed = view.desired !== 'none' && view.desired !== 'sim';
  const named = ports.find((port) => port.name === view.endpoint);

  return <section className="link-section" data-av-surface="transport">
    <Panel title="Link" right={<span className="panel-route-control"><span>Route</span><EnumSelect path="/motion/route" /></span>}>
      <LinkState connected={view.connected} observed={observed} detail={view.detail} />
      {observed === 'sim' && <Banner tone="warn">This is the simulated module, not hardware. Runs are real runs of the engine and the decoder, and are stamped <code>transport: sim</code> in the report — they are not evidence about a board.</Banner>}
      {view.connected ? <div className="link-facts">
        <Fact label={rs485 ? 'Transport' : 'Protocol'} value={observed} />
        {addressed && <Fact label={rs485 ? 'Endpoint' : 'Port'} value={view.endpoint || '—'} />}
        {named?.product && <Fact label="Device" value={named.product} />}
      </div> : <div className="link-choices">
        <Row label={rs485 ? 'Transport' : 'Protocol'}>
          {rs485
            ? <FriendlyEnum path="/rs485/desired" labels={RS485_LABELS} hide={simulated ? undefined : ['sim']} />
            : <FriendlyEnum path="/serial/desired" labels={SERIAL_LABELS} />}
        </Row>
        {addressed && <EndpointPicker path={rs485 ? '/rs485/endpoint' : '/serial/port'} transport={view.desired} probeSerial={probeSerial} />}
      </div>}

      {rs485 && <div className="target-row">
        <Row label="Target address" hint="1–127 · applied as you change it"><NumberField path="/rs485/target" /></Row>
        <Action path="/actions/select_rs485_target" why={!rs485Connected ? 'RS485 is not connected' : null}>Re-select</Action>
      </div>}

      <div className="button-row">
        {rs485 ? <>
          <Action path="/actions/connect_rs485" variant="primary" why={connectWhy}>Connect</Action>
          <Action path="/actions/disconnect_rs485" why={!rs485Connected ? 'not connected' : null}>Disconnect</Action>
          <Action path="/actions/discover_rs485" why={!rs485Connected ? 'not connected' : null}>Discover</Action>
          <Action path="/actions/identify_rs485" why={!rs485Connected ? 'not connected' : null}>Identify target</Action>
        </> : <>
          <Action path="/actions/connect_serial" variant="primary" why={connectWhy}>Connect</Action>
          <Action path="/actions/disconnect_serial" why={!serialConnected ? 'not connected' : null}>Disconnect</Action>
          <Action path="/actions/identify_serial" why={!serialConnected ? 'not connected' : null}>Identify</Action>
        </>}
      </div>

      {rs485 && <div className="rs485-evidence"><Fact label="Discovered" value={useText('/rs485/discovered') || '—'} /><Fact label="ACKs / timeouts" value={`${useNumber('/rs485/stats/acks')} / ${useNumber('/rs485/stats/ack_timeouts')}`} /><Fact label="RX / TX" value={`${useNumber('/rs485/stats/rx')} / ${useNumber('/rs485/stats/tx')}`} /><Fact label="Decode / queued" value={`${useNumber('/rs485/stats/decode_errors')} / ${useNumber('/rs485/stats/outbox')}`} /></div>}

      {rs485
        ? <OtherRoute name="serial" connected={serialConnected} observed={serialObserved} endpoint={serialPort} />
        : <OtherRoute name="RS485" connected={rs485Connected} observed={rs485Observed} endpoint={rs485Endpoint} />}
    </Panel>
    {blocker && <Banner tone="info">{blocker}</Banner>}
  </section>;
}

function AxisFacts({ axis }: { axis: 'a' | 'b' }) {
  const known = useBool(`/dut/${axis}/position_known`);
  const p = useNumber(`/dut/${axis}/position`); const t = useNumber(`/dut/${axis}/target`);
  return <div className="axis-fact"><strong>Axis {axis.toUpperCase()}</strong><Fact label="Measured" value={known ? `${p.toLocaleString()} µsteps` : '—'} /><Fact label="Target" value={known ? `${t.toLocaleString()} µsteps` : '—'} /><Fact label="Error" value={known ? `${(t-p).toLocaleString()} µsteps` : '—'} tone={known && t !== p ? 'warn' : undefined} /></div>;
}

function MotionControl() {
  const route = useEnumName('/motion/route');
  const rs485Connected = useBool('/rs485/connected');
  const serialConnected = useBool('/serial/connected');
  const connected = route === 'rs485' ? rs485Connected : serialConnected;
  const usteps = useNumber('/dut/usteps_per_rev');
  const a = useNumber('/motion/a/rotations'), b = useNumber('/motion/b/rotations');
  return <section className="motion-section" data-av-surface="live-telemetry">
    <Panel title="Motion control" right={<Badge tone={connected ? 'ok' : 'offline'}>via {route}</Badge>}>
      <MotionPilot />
      <div className="axis-targets"><Fact label="A exact target" value={usteps ? `${Math.round(a * usteps).toLocaleString()} µsteps` : 'identify first'} /><Fact label="B exact target" value={usteps ? `${Math.round(-b * usteps).toLocaleString()} µsteps` : 'identify first'} /></div>
      <Action path="/actions/motion_push" variant="primary" className="push-motion" why={!connected ? `${route} is not connected` : !usteps ? 'identify or home first' : null}>Move axes</Action>
      <div className="axis-cards"><AxisFacts axis="a" /><AxisFacts axis="b" /></div>
    </Panel>
    <Panel title="Measured motion"><MotionGraphs /></Panel>
  </section>;
}

function Evidence() {
  const reason = useText('/last/reason');
  const faults = useNumber('/faults/active');
  return <section className="evidence-grid">
    <Panel title="Last test result"><Fact label="Plan" value={useText('/last/plan') || '—'} /><Fact label="Verdict" value={useEnumName('/last/verdict')} /><Fact label="Measurements" value={useText('/last/measurements') || '—'} />{reason && <Banner tone="warn">{reason}</Banner>}<Fact label="Report" value={useText('/last/report_path') || '—'} /><div data-av-surface="faults"><Fact label="Active faults" value={faults} tone={faults ? 'error' : undefined} /></div></Panel>
    <Panel title="Bench setup"><ParamTree prefix="/setup" /></Panel>
  </section>;
}

function TestTab() {
  return <>
    <LinkPanel />
    <ProcedureRunner />
    <QuickCommands />
    <HomingDiagnostics />
    <MotionControl />
    <RawSignalPanel />
    <Evidence />
  </>;
}

function useHeartbeat() { const p = useParam<number>('/ui/heartbeat'); useEffect(() => { const id = setInterval(() => p.set(Date.now()), 1000); return () => clearInterval(id); }, [p.set]); }

function useCueSounds(sounds: SystemSounds, cue: Cue, seq: number) {
  const seen = useRef<number | null>(null);
  useEffect(() => {
    if (seen.current === null) { seen.current = seq; return; }
    if (seen.current === seq) return;
    seen.current = seq;
    const action = soundFor(cue);
    const eventId = `portal-test-bench:${cue}:${seq}`;
    if (action.kind === 'loop') sounds.process('idle', eventId);
    else if (action.kind === 'play') { sounds.stopIdle(); sounds.play(action.name, eventId); }
  }, [sounds, cue, seq]);
}

function App() {
  const schema = useSchema(); useHeartbeat();
  const sounds = useMemo(() => new SystemSounds(), []);
  const [soundEnabled, setSoundEnabled] = useState(() => !sounds.muted);
  const cue = useEnumName('/cue') as Cue;
  const cueSeq = useNumber('/cue_seq');
  useCueSounds(sounds, cue, cueSeq);
  useEffect(() => () => sounds.dispose(), [sounds]);
  const changeSoundEnabled = (enabled: boolean) => {
    sounds.setMuted(!enabled);
    setSoundEnabled(enabled);
    if (enabled) sounds.play('tick_small', `portal-test-bench:sound-enabled:${Date.now()}`);
  };
  const [tab, setTab] = useState<'flash' | 'database' | 'test' | 'inspect'>('flash');
  const serial = useBool('/serial/connected'), rs485 = useBool('/rs485/connected');
  const serialKind = useEnumName('/serial/observed');
  const rs485Target = useNumber('/rs485/target');
  const target = useBool('/probe/target_present'), probe = useBool('/probe/connected');
  const passed = useNumber('/counts/passed'), failed = useNumber('/counts/failed'), faults = useNumber('/faults/active');
  return <div className="app app--filled bench">
    {/* Once, and outside the tabs: the gesture is "drop it on the bench", not "find the firmware
        panel first". A drop while the Test tab is open still lands in the right bank. */}
    <FirmwareDrop />
    <TitleBar title="Portal Test Bench" sub={schema ? 'flashing · communications · motion diagnostics' : 'connecting'} />
    <div className="bench-main"><div className="bench-workspace">
      <div className="bench-shared">
        <HardwareBand />
        <FlashActionStrip soundEnabled={soundEnabled} onSoundEnabledChange={changeSoundEnabled} />
        <Tabs value={tab} onChange={setTab} label="Portal test bench workspaces" items={[{ id: 'flash', label: 'Flash' }, { id: 'database', label: 'Database' }, { id: 'test', label: 'Test', count: faults || undefined }, { id: 'inspect', label: 'Inspect' }]} />
      </div>
      <div className="tab-content"><div className="stack bench-stack">{tab === 'flash' ? <FlashTab /> : tab === 'database' ? <DatabaseTab /> : tab === 'test' ? <TestTab /> : <InspectTab />}</div></div>
    </div><SessionLog /></div>
    <StatusBar stream={null}><StatusItem label="serial" value={serial ? serialKind : 'down'} tone={serial ? 'ok' : 'warn'} /><StatusItem label="RS485" value={rs485 ? `target ${rs485Target}` : 'down'} tone={rs485 ? 'ok' : 'warn'} /><StatusItem label="probe" value={target ? 'MCU present' : probe ? 'ready' : 'down'} tone={target ? 'ok' : probe ? 'warn' : 'error'} /><StatusItem label="runs" value={`${passed} pass · ${failed} fail`} />{faults > 0 && <StatusItem label="faults" value={String(faults)} tone="error" />}</StatusBar>
  </div>;
}

mount(<App />);
