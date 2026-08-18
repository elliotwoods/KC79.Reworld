import {
  Badge, Banner, Button, EmptyState, EnumSelect, NumberField, Panel, ParamTree, Row, StatusBar, StatusItem,
  TextField, TitleBar, Toggle,
} from '@auroravision/av-gui/controls';
import { mount, useParam, useSchema } from '@auroravision/av-gui/runtime';
import '@auroravision/av-gui/styles.css';
import { useEffect, useState, type ReactNode } from 'react';
import { SessionLog } from './bench-log';
import { MotionGraphs, MotionPilot } from './motion';

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
  const flashDetail = useText('/flash/detail');
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
  return <section className="hardware-band" data-av-surface="module-state">
    <div className="hardware-status"><span className={`presence-dot ${probe && target ? 'is-ok' : 'is-warn'}`} /><div><strong>{target ? 'MCU connected' : probe ? 'ST-Link ready · no target' : 'No ST-Link'}</strong><small>{flashDetail || (dut ? 'communications online' : 'waiting for hardware')}</small></div></div>
    <div className="hardware-facts">
      <Fact label="Firmware" value={mcuFirmware || dutVersion || '—'} />
      <Fact label="ST-Link" value={[probeName, probeFirmware].filter(Boolean).join(' · ') || '—'} />
      <Fact label="MCU" value={mcuPart || '—'} />
      <Fact label="UID" value={mcuUid || '—'} />
      <Fact label="IDCODE / DEV_ID" value={[mcuIdcode, mcuDevId].filter(Boolean).join(' / ') || '—'} />
      <Fact label="Flash" value={flashKb > 0 ? `${flashKb} kB · ${mcuLayout} · RDP ${mcuRdp}` : '—'} />
    </div>
  </section>;
}

interface ProbeChoice { identifier: string; name?: string; serial_number?: string; kind: string }
interface Artefact { id: string; label: string; region: 'bootloader' | 'application'; origin: string; bytes: number; fits: boolean }
interface MissingArtefact { label: string; path: string; hint: string }

function ChoiceRow({ selected, disabled = false, title, detail, badges, onClick }: { selected: boolean; disabled?: boolean; title: string; detail: string; badges: ReactNode; onClick: () => void }) {
  return <button type="button" role="option" aria-selected={selected} data-selected={selected} data-disabled={disabled || undefined} className="choice-row" disabled={disabled} onClick={onClick}>
    <span className="choice-mark" aria-hidden="true">{selected ? '✓' : ''}</span>
    <span className="choice-copy"><strong>{title}</strong><small>{detail}</small></span>
    <span className="choice-badges">{badges}</span>
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
            <ChoiceRow key={item.identifier} selected={probe.value === item.identifier} disabled={setupLocked} title={item.name || item.identifier} detail={[item.kind, item.serial_number || item.identifier].filter(Boolean).join(' · ')} badges={<Badge tone={probe.value === item.identifier ? 'active' : 'idle'}>{probe.value === item.identifier ? 'selected' : 'available'}</Badge>} onClick={() => probe.set(item.identifier)} />
          )}</div>}
        {probeChoices.length > 1 && !probe.value && <Banner tone="warn">Multiple probes are attached. Select the one wired to this fixture; the bench will not guess.</Banner>}
        {selectedProbeMissing && <Banner tone="warn">The selected probe is no longer attached. Rescan or choose another.</Banner>}
        <footer>{useText('/probe/name') || 'No probe open'}<span>{useText('/probe/firmware') || '—'} · {useNumber('/probe/speed_khz') || 0} kHz</span></footer>
      </section>
      <section className="setup-choice-group" aria-label="Firmware selection">
        <header><span><b>2</b> Firmware banks</span><Badge tone={scope === 'full' ? 'ok' : scope === 'nothing' ? 'offline' : 'active'}>{scope}</Badge></header>
        {items.length === 0 ? <EmptyState inline detail="No production firmware was found in the build tree." /> :
          <div className="choice-list" role="listbox" aria-label="Firmware artifacts">{items.map((item) => {
            const selection = item.region === 'bootloader' ? boot : app;
            const selected = selection.value === item.id;
            return <ChoiceRow key={item.id} selected={selected} disabled={!item.fits || setupLocked} title={item.label} detail={`${item.origin} · ${(item.bytes / 1024).toFixed(1)} kB`} badges={<><Badge tone="idle">{item.region}</Badge><Badge tone={!item.fits ? 'error' : selected ? 'active' : 'idle'}>{!item.fits ? 'too large' : selected ? 'selected' : 'available'}</Badge></>} onClick={() => selection.set(selected ? '' : item.id)} />;
          })}</div>}
        {setupLocked && <Banner tone="info">Selection is locked while flashing, auto-flash, or a test plan is active.</Banner>}
        {missing.map((item) => <Banner key={item.path} tone="info">{item.label}: {item.hint}</Banner>)}
        <footer title={root}><span>Click a selected bank to leave it out</span><code>{root || 'build tree unavailable'}</code></footer>
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
  return <span className="hero-action" title={why ?? 'Press twice within five seconds'}><Button variant={confirmUntil > Date.now() ? 'danger' : 'primary'} disabled={!!why || !action.decl} onClick={click}>{confirmUntil > Date.now() ? 'Confirm flash now' : 'Flash manually'}</Button></span>;
}

function Operations() {
  const progress = useNumber('/flash/progress');
  const busy = useBool('/flash/busy');
  const armed = useBool('/flash/armed');
  const last = useText('/flash/last_outcome');
  const step = useText('/flash/step');
  const phase = useText('/flash/phase');
  const scope = useText('/flash/scope');
  const simPresent = useParam<boolean>('/sim/module_present');
  return <Panel title="Operations">
    <div className="flash-operations" data-av-surface="firmware">
      <SetupPicker />
      <div className="flash-actions"><ManualFlashButton /><div className={`auto-card${armed ? ' is-armed' : ''}`}><div><strong>Automatic fixture flashing</strong><small>{armed ? 'Armed · remove the board after PASS' : 'Insert → flash → cycle → run-check'}</small></div><Toggle path="/flash/auto_enabled" /></div></div>
      {simPresent.decl && <div className="simulation-fixture"><span>Simulated board in fixture</span><Toggle path="/sim/module_present" /></div>}
      <div className="flash-progress"><div style={{ width: `${Math.round(progress * 100)}%` }} /><span>{busy ? `${step || phase} · ${Math.round(progress * 100)}%` : last || `${scope} selected`}</span></div>
    </div>
    <div className="secondary-operations">
      <Action path="/actions/startup" variant="primary">Run startup test</Action>
      <Action path="/actions/abort" variant="danger">Abort</Action>
      <Action path="/actions/escape">Escape routine</Action>
      <Action path="/actions/calibrate_threshold">Calibrate threshold</Action>
      <Action path="/actions/home_a">Home A</Action><Action path="/actions/home_b">Home B</Action>
      <Action path="/actions/unjam_a">Unjam A</Action><Action path="/actions/unjam_b">Unjam B</Action>
      <Action path="/actions/read_device">Read device</Action><Action path="/actions/marker">Drop marker</Action>
    </div>
  </Panel>;
}

function LinkState({ connected, observed, detail }: { connected: boolean; observed: string; detail: string }) {
  return <div className="link-state"><Badge tone={connected ? 'ok' : 'offline'}>{connected ? observed : 'down'}</Badge><span>{detail || (connected ? 'receiving evidence' : 'not connected')}</span></div>;
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
    <Panel title="Motion control" right={<EnumSelect path="/motion/route" />}>
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

function useHeartbeat() { const p = useParam<number>('/ui/heartbeat'); useEffect(() => { const id = setInterval(() => p.set(Date.now()), 1000); return () => clearInterval(id); }, [p.set]); }

function App() {
  const schema = useSchema(); useHeartbeat();
  const serial = useBool('/serial/connected'), rs485 = useBool('/rs485/connected');
  const serialKind = useEnumName('/serial/observed');
  const rs485Target = useNumber('/rs485/target');
  const target = useBool('/probe/target_present'), probe = useBool('/probe/connected');
  const flashBusy = useBool('/flash/busy');
  const runBusy = useBool('/run/busy');
  const busy = flashBusy || runBusy;
  const passed = useNumber('/counts/passed'), failed = useNumber('/counts/failed'), faults = useNumber('/faults/active');
  return <div className="app app--filled bench">
    <TitleBar title="Portal Test Bench" sub={schema ? 'flashing · communications · motion diagnostics' : 'connecting'} />
    <div className="bench-main"><div className="stack bench-stack">
      <HardwareBand />
      <section className={`readiness ${busy ? 'is-busy' : target ? 'is-ready' : 'is-waiting'}`} data-av-surface="test-runner"><strong>{busy ? 'WORKING' : target ? 'READY' : probe ? 'WAITING FOR MCU' : 'WAITING FOR ST-LINK'}</strong><span>{useText('/flash/phase')} {useText('/flash/last_outcome')}</span></section>
      <Operations /><TransportPanels /><MotionControl /><Evidence />
    </div><SessionLog /></div>
    <StatusBar stream={null}><StatusItem label="serial" value={serial ? serialKind : 'down'} tone={serial ? 'ok' : 'warn'} /><StatusItem label="RS485" value={rs485 ? `target ${rs485Target}` : 'down'} tone={rs485 ? 'ok' : 'warn'} /><StatusItem label="probe" value={target ? 'MCU present' : probe ? 'ready' : 'down'} tone={target ? 'ok' : probe ? 'warn' : 'error'} /><StatusItem label="runs" value={`${passed} pass · ${failed} fail`} />{faults > 0 && <StatusItem label="faults" value={String(faults)} tone="error" />}</StatusBar>
  </div>;
}

mount(<App />);
