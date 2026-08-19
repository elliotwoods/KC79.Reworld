import {
  Badge, Banner, Button, EmptyState, EnumSelect, NumberField, Panel, ParamTree, Row, StatusBar, StatusItem,
  Tabs, TextField, TitleBar, Toggle,
} from '@auroravision/av-gui/controls';
import { SystemSounds } from '@auroravision/av-gui/calibration';
import { mount, useParam, useSchema } from '@auroravision/av-gui/runtime';
import '@auroravision/av-gui/styles.css';
import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { type Cue, soundFor } from './bench-model';
import { SessionLog } from './bench-log';
import { MotionGraphs, MotionPilot } from './motion';
import { InspectTab } from './inspect';

function useEnumName(path: string): string {
  const p = useParam<number>(path);
  return p.decl?.variants.find((v) => v.value === p.value)?.name ?? 'unknown';
}
const useText = (path: string) => useParam<string>(path).value ?? '';
const useNumber = (path: string) => useParam<number>(path).value ?? 0;
const useBool = (path: string) => !!useParam<boolean>(path).value;

function Action({ path, children, why, variant, className }: { path: string; children: ReactNode; why?: string | null; variant?: 'default' | 'primary' | 'danger' | 'quiet'; className?: string }) {
  const p = useParam<number>(path);
  const disabled = !!why || !p.decl;
  return <span className={className} title={why ?? p.decl?.label ?? path}><Button variant={variant} disabled={disabled} onClick={() => p.set((p.value ?? 0) + 1)}>{children}</Button></span>;
}

function Fact({ label, value, tone }: { label: string; value: ReactNode; tone?: string }) {
  return <div className={`fact${tone ? ` is-${tone}` : ''}`}><span className="fact-label">{label}</span><span className="fact-value">{value}</span></div>;
}

function FriendlyEnum({ path, labels }: { path: string; labels: Record<string, string> }) {
  const p = useParam<number>(path);
  return <select className="friendly-select" value={p.value ?? 0} disabled={!p.decl || p.readOnly} onChange={(e) => p.set(Number(e.target.value))} aria-label={p.decl?.label ?? path}>
    {(p.decl?.variants ?? []).map((v) => <option key={v.value} value={v.value}>{labels[v.name] ?? v.name}</option>)}
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

interface ProbeChoice { identifier: string; name?: string; serial_number?: string; kind: string }
interface Artefact { id: string; label: string; region: 'bootloader' | 'application'; origin: string; bytes: number; fits: boolean }
interface MissingArtefact { label: string; path: string; hint: string }

function ChoiceRow({ selected, disabled = false, title, detail, badges, onClick }: { selected: boolean; disabled?: boolean; title: string; detail: string; badges?: ReactNode; onClick: () => void }) {
  return <button type="button" role="option" aria-selected={selected} data-selected={selected} data-disabled={disabled || undefined} className="choice-row" disabled={disabled} onClick={onClick}>
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
  const [probes, setProbes] = useState<ProbeChoice[]>([]);
  const [items, setItems] = useState<Artefact[]>([]);
  const [missing, setMissing] = useState<MissingArtefact[]>([]);
  const [root, setRoot] = useState('');
  const [loading, setLoading] = useState(false);
  const load = async () => {
    setLoading(true);
    try {
      const [firmwareResponse, portsResponse] = await Promise.all([
        fetch('/api/bench/firmware', { cache: 'no-store' }),
        fetch('/api/bench/ports', { cache: 'no-store' }),
      ]);
      if (firmwareResponse.ok) {
        const firmware = await firmwareResponse.json();
        setItems(firmware.found ?? []);
        setMissing(firmware.missing ?? []);
        setRoot(firmware.root ?? '');
      }
      if (portsResponse.ok) setProbes((await portsResponse.json()).probes ?? []);
    } catch {
      // The host may be restarting; the explicit rescan and the next page load retry.
    } finally {
      setLoading(false);
    }
  };
  useEffect(() => { void load(); }, []);
  const probeChoices = simulated
    ? [{ identifier: 'sim', name: 'SimRig', serial_number: 'SIM', kind: 'simulation' }]
    : probes;
  const selectedProbeMissing = !!probe.value && !probeChoices.some((item) => item.identifier === probe.value);
  const setupLocked = armed || flashBusy || runBusy;
  const doRescan = () => {
    rescan.set((rescan.value ?? 0) + 1);
    window.setTimeout(() => void load(), 150);
  };
  return <div className="setup-picker">
    <div className="setup-picker-toolbar">
      <div><strong>Fixture setup</strong><small>Choose hardware first, then the image banks to program.</small></div>
      <Button variant="quiet" disabled={loading || setupLocked} onClick={doRescan}>{loading ? 'Scanning…' : 'Rescan all'}</Button>
    </div>
    <div className="setup-picker-columns">
      <section className="setup-choice-group" aria-label="Probe selection">
        <header><span><b>1</b> ST-Link probe</span><Badge tone={connected ? 'ok' : 'offline'}>{connected ? 'connected' : 'not connected'}</Badge></header>
        {probeChoices.length === 0 ? <EmptyState inline detail="No ST-Link found. Connect the fixture probe and rescan." /> :
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
            const regionItems = items.filter((item) => item.region === region);
            return <section key={region} className="firmware-bank" aria-label={`${region} bank`}>
              <strong>{region === 'bootloader' ? 'Bootloader' : 'Application'}</strong>
              {regionItems.length === 0 ? <small>No {region} image found.</small> : <div className="choice-list" role="listbox" aria-label={`${region} firmware`}>{regionItems.map((item) => {
                const selected = selection.value === item.id;
                return <ChoiceRow key={item.id} selected={selected} disabled={!item.fits || setupLocked} title={item.label} detail={`${item.origin} · ${(item.bytes / 1024).toFixed(1)} kB`} badges={!item.fits ? <Badge tone="error">too large</Badge> : undefined} onClick={() => selection.set(selected ? '' : item.id)} />;
              })}</div>}
            </section>;
          })}</div>}
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
  const [confirmUntil, setConfirmUntil] = useState(0);
  const why = busy ? 'a flash pass is running' : runBusy ? 'a test plan is running' : armed ? 'disable auto-flash first' : !probe ? 'no ST-Link' : !target ? 'no MCU is answering' : null;
  const click = () => {
    const now = Date.now();
    if (now < confirmUntil) { action.set((action.value ?? 0) + 1); setConfirmUntil(0); }
    else { setConfirmUntil(now + 5000); window.setTimeout(() => setConfirmUntil((v) => v <= Date.now() ? 0 : v), 5100); }
  };
  return <span className="manual-flash" title={why ?? 'Press twice within five seconds'}><Button variant={confirmUntil > Date.now() ? 'danger' : 'primary'} disabled={!!why || !action.decl} onClick={click}>{confirmUntil > Date.now() ? 'Confirm flash' : 'Flash / Provision now'}</Button></span>;
}

function SerialNumberControl() {
  const requested = useNumber('/provision/serial_to_provision');
  const existing = useNumber('/provision/on_board_serial');
  const identity = useText('/provision/identity_state');
  const dbOk = useBool('/provision/database_ok');
  const pending = useBool('/provision/pending_replug');
  const differs = existing > 0 && requested !== existing;
  const detail = pending
    ? 'reserved; waiting for replug'
    : differs
      ? `on-board serial: ${existing}`
      : existing > 0
        ? 'matches on-board serial'
        : identity === 'blank'
          ? 'new board'
          : identity || 'identity unknown';
  return <div className={`serial-number-control${differs ? ' is-different' : ''}${!dbOk ? ' is-error' : ''}`}>
    <span className="serial-number-copy"><span className="label-caps">Serial Number</span><small>{detail}</small></span>
    <NumberField path="/provision/serial_to_provision" />
    <Badge tone={!dbOk ? 'error' : pending || differs ? 'warn' : existing > 0 ? 'ok' : 'idle'}>{!dbOk ? 'DB offline' : pending ? 'pending' : differs ? 'review' : existing > 0 ? 'existing' : 'new'}</Badge>
  </div>;
}

function FlashActionStrip({ soundEnabled, onSoundEnabledChange }: { soundEnabled: boolean; onSoundEnabledChange: (enabled: boolean) => void }) {
  const probe = useBool('/probe/connected');
  const target = useBool('/probe/target_present');
  const flashBusy = useBool('/flash/busy');
  const runBusy = useBool('/run/busy');
  const armed = useBool('/flash/armed');
  const forceWrite = useParam<boolean>('/flash/force_write');
  const autoFlash = useParam<boolean>('/flash/auto_enabled');
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
  const toggleForceWrite = () => {
    const enabled = !forceWrite.value;
    if (enabled && !autoFlash.value) autoFlash.set(true);
    forceWrite.set(enabled);
  };
  return <section className={`action-strip ${busy ? 'is-busy' : target ? 'is-ready' : 'is-waiting'}`} data-av-surface="test-runner">
    {busy && <span className="action-progress" style={{ width: `${Math.round(progress * 100)}%` }} />}
    <div className="action-state"><strong>{state}</strong>{busy && <span>{detail} · {Math.round(progress * 100)}%</span>}</div>
    <SerialNumberControl />
    <ManualFlashButton />
    <div className={`auto-flash${armed ? ' is-armed' : ''}`}><span><strong>Auto flash</strong>{armed && <small>Armed</small>}</span><button type="button" className="sound-toggle" aria-label={soundEnabled ? 'Disable auto-flash sounds' : 'Enable auto-flash sounds'} aria-pressed={soundEnabled} title={soundEnabled ? 'Sound on' : 'Sound off'} onClick={() => onSoundEnabledChange(!soundEnabled)}><span aria-hidden="true">{soundEnabled ? '🔊' : '🔇'}</span></button><button type="button" className={`force-toggle${forceWrite.value ? ' is-active' : ''}`} aria-label={forceWrite.value ? 'Disable forced firmware writes' : 'Force firmware writes even when the image matches'} aria-pressed={!!forceWrite.value} title="Bypass the matching-firmware skip" disabled={!forceWrite.decl || (!forceWrite.value && !autoFlash.decl) || flashBusy} onClick={toggleForceWrite}>Force</button><Toggle path="/flash/auto_enabled" /></div>
  </section>;
}

interface ProvisionActionRow { id: number; at_ms: number; serial?: number; uid?: string; action: string; outcome: string; detail: string }

function ProvisionPanel() {
  const dbOk = useBool('/provision/database_ok');
  const dbError = useText('/provision/database_error');
  const identity = useText('/provision/identity_state');
  const existing = useNumber('/provision/on_board_serial');
  const pending = useBool('/provision/pending_replug');
  const reservation = useText('/provision/reservation');
  const source = useText('/provision/settings/source');
  const [query, setQuery] = useState('');
  const [history, setHistory] = useState<ProvisionActionRow[]>([]);
  useEffect(() => {
    let active = true;
    const load = async () => {
      try {
        const response = await fetch(`/api/bench/provision/history?q=${encodeURIComponent(query)}`, { cache: 'no-store' });
        if (active && response.ok) setHistory((await response.json()).actions ?? []);
      } catch { /* the host may be restarting */ }
    };
    void load();
    const id = window.setInterval(load, 2000);
    return () => { active = false; window.clearInterval(id); };
  }, [query]);
  return <section className="provision-panel" aria-label="Board provisioning">
    <header><div><strong>Board provisioning</strong><small>Serial identity is durable and separate from the 1–127 RS485 address.</small></div><Badge tone={!dbOk ? 'error' : pending ? 'warn' : existing > 0 ? 'ok' : 'idle'}>{!dbOk ? 'database unavailable' : pending ? 'waiting for replug' : identity}</Badge></header>
    {!dbOk && <Banner tone="error">Provisioning is blocked: {dbError || 'the local database is unavailable'}. Diagnostics and tests remain usable.</Banner>}
    {pending && <Banner tone="warn">Remove and reconnect this board. Its pending UID, serial, and firmware will be verified without another flash.</Banner>}
    <div className="provision-grid">
      <section className="provision-fields"><header><strong>Serial allocation</strong><small>The Serial Number above is the number printed on the PCB.</small></header><Row label="Next available serial number"><NumberField path="/provision/next_serial" /></Row><Fact label="On-board serial number" value={existing > 0 ? String(existing) : 'none'} /><Fact label="Identity status" value={identity || 'unknown'} tone={identity === 'corrupt' || identity === 'conflicted' ? 'error' : undefined} /><Fact label="Reservation" value={reservation || 'none'} /><div className="button-row"><Action path="/actions/keep_onboard_serial" why={existing <= 0 ? 'no valid on-board serial' : null}>Keep on-board serial</Action><Action path="/actions/use_pcb_serial" variant="danger">Use entered serial number</Action></div></section>
      <section className="provision-fields"><header><strong>Module flash settings</strong><small>Written to the persistent settings journal.</small></header><Row label="Operating current [mA]"><NumberField path="/provision/settings/current_ma" /></Row><Row label="Full-current home recovery"><Toggle path="/provision/settings/recovery_enabled" /></Row><Fact label="Settings source" value={source || 'defaults'} /><div className="button-row"><Action path="/actions/read_settings">Read from board</Action><Action path="/actions/write_settings" variant="primary">Write to board</Action></div></section>
    </div>
    <div className="provision-history"><header><strong>Provisioning history</strong><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Search serial, UID, action…" aria-label="Search provisioning history" /></header>{history.length === 0 ? <small>No matching actions.</small> : <div className="history-list">{history.slice(0, 30).map((item) => <div key={item.id}><code>{item.serial ?? '—'}</code><span><strong>{item.action}</strong><small>{item.uid ? `…${item.uid.slice(-8)} · ` : ''}{item.detail || item.outcome}</small></span><Badge tone={item.outcome === 'failed' ? 'error' : item.outcome === 'ok' ? 'ok' : 'idle'}>{item.outcome}</Badge></div>)}</div>}</div>
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
    if (required === 'rs485' && !observed.startsWith('rs485')) return 'requires an RS485 link';
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
  const vcom = kind === 'vcp' || kind === 'sim';
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

function TransportPanels() {
  const serial = useBool('/serial/connected');
  const rs485 = useBool('/rs485/connected');
  return <section className="transport-grid" data-av-surface="transport">
    <Panel title="Serial console">
      <LinkState connected={serial} observed={useEnumName('/serial/observed')} detail={useText('/serial/detail')} />
      <Row label="Protocol"><FriendlyEnum path="/serial/desired" labels={{ none: 'Choose protocol', vcp: 'Production serial console', 'bench-ascii': 'Bench firmware console' }} /></Row>
      <Row label="Port"><TextField path="/serial/port" /></Row>
      <div className="button-row"><Action path="/actions/connect_serial" why={serial ? 'already connected' : null}>Connect</Action><Action path="/actions/disconnect_serial" why={!serial ? 'not connected' : null}>Disconnect</Action><Action path="/actions/identify_serial" why={!serial ? 'not connected' : null}>Identify</Action></div>
    </Panel>
    <Panel title="RS485 bus">
      <LinkState connected={rs485} observed={useEnumName('/rs485/observed')} detail={useText('/rs485/detail')} />
      <Row label="Transport"><FriendlyEnum path="/rs485/desired" labels={{ none: 'Choose transport', 'rs485-serial': 'USB / serial adapter', 'rs485-tcp': 'Ethernet gateway' }} /></Row>
      <Row label="Endpoint"><TextField path="/rs485/endpoint" /></Row>
      <div className="target-row"><Row label="Target address"><NumberField path="/rs485/target" /></Row><Action path="/actions/select_rs485_target" why={!rs485 ? 'RS485 is not connected' : null}>Select</Action></div>
      <div className="button-row"><Action path="/actions/connect_rs485" why={rs485 ? 'already connected' : null}>Connect</Action><Action path="/actions/disconnect_rs485" why={!rs485 ? 'not connected' : null}>Disconnect</Action><Action path="/actions/discover_rs485" why={!rs485 ? 'not connected' : null}>Discover</Action><Action path="/actions/identify_rs485" why={!rs485 ? 'not connected' : null}>Identify target</Action></div>
      <div className="rs485-evidence"><Fact label="Discovered" value={useText('/rs485/discovered') || '—'} /><Fact label="ACKs / timeouts" value={`${useNumber('/rs485/stats/acks')} / ${useNumber('/rs485/stats/ack_timeouts')}`} /><Fact label="RX / TX" value={`${useNumber('/rs485/stats/rx')} / ${useNumber('/rs485/stats/tx')}`} /><Fact label="Decode / queued" value={`${useNumber('/rs485/stats/decode_errors')} / ${useNumber('/rs485/stats/outbox')}`} /></div>
    </Panel>
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
    <Panel title="Motion control" right={<span className="panel-route-control"><span>Route</span><EnumSelect path="/motion/route" /></span>}>
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
  const route = useEnumName('/motion/route');
  const serialConnected = useBool('/serial/connected');
  const rs485Connected = useBool('/rs485/connected');
  const connected = route === 'rs485' ? rs485Connected : serialConnected;
  return <>
    <section className="test-route">
      <div><span className="label-caps">Test route</span><strong>{route === 'rs485' ? 'RS485 addressed bus' : 'VCOM / serial'}</strong><small>Both links can stay connected; this selects where procedures and commands go.</small></div>
      <EnumSelect path="/motion/route" />
      <Badge tone={connected ? 'ok' : 'offline'}>{connected ? 'connected' : 'not connected'}</Badge>
    </section>
    <ProcedureRunner />
    <QuickCommands />
    <HomingDiagnostics />
    <TransportPanels />
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
  const [tab, setTab] = useState<'flash' | 'test' | 'inspect'>('flash');
  const serial = useBool('/serial/connected'), rs485 = useBool('/rs485/connected');
  const serialKind = useEnumName('/serial/observed');
  const rs485Target = useNumber('/rs485/target');
  const target = useBool('/probe/target_present'), probe = useBool('/probe/connected');
  const passed = useNumber('/counts/passed'), failed = useNumber('/counts/failed'), faults = useNumber('/faults/active');
  return <div className="app app--filled bench">
    <TitleBar title="Portal Test Bench" sub={schema ? 'flashing · communications · motion diagnostics' : 'connecting'} />
    <div className="bench-main"><div className="bench-workspace">
      <div className="bench-shared">
        <HardwareBand />
        <FlashActionStrip soundEnabled={soundEnabled} onSoundEnabledChange={changeSoundEnabled} />
        <Tabs value={tab} onChange={setTab} label="Portal test bench workspaces" items={[{ id: 'flash', label: 'Flash' }, { id: 'test', label: 'Test', count: faults || undefined }, { id: 'inspect', label: 'Inspect' }]} />
      </div>
      <div className="tab-content"><div className="stack bench-stack">{tab === 'flash' ? <FlashTab /> : tab === 'test' ? <TestTab /> : <InspectTab />}</div></div>
    </div><SessionLog /></div>
    <StatusBar stream={null}><StatusItem label="serial" value={serial ? serialKind : 'down'} tone={serial ? 'ok' : 'warn'} /><StatusItem label="RS485" value={rs485 ? `target ${rs485Target}` : 'down'} tone={rs485 ? 'ok' : 'warn'} /><StatusItem label="probe" value={target ? 'MCU present' : probe ? 'ready' : 'down'} tone={target ? 'ok' : probe ? 'warn' : 'error'} /><StatusItem label="runs" value={`${passed} pass · ${failed} fail`} />{faults > 0 && <StatusItem label="faults" value={String(faults)} tone="error" />}</StatusBar>
  </div>;
}

mount(<App />);
