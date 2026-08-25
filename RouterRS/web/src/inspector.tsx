// The right-hand inspector: context follows `/ui/select/*`. Installation → arrangement and
// messaging; column → RS485 link, scheduled poll, column-scope actions and pad; portal →
// header + Pilot / Axis A / Axis B / Motor / Log sub-panels, matching the iced inspector
// and the C++ ofxCvGui one before it.

import {
  Badge,
  Button,
  EnumSelect,
  NumberField,
  Panel,
  Row,
  Tabs,
  Toggle,
} from '@auroravision/av-gui/controls';
import { Sparkline } from '@auroravision/av-gui/charts';
import { useParam, useTelemetry } from '@auroravision/av-gui/runtime';
import { useEffect, useState } from 'react';
import { Action, BroadcastActions, Fact } from './bits';
import { AxisDial, PilotAllPad, PilotDisk, SEL } from './canvas';
import { FirmwarePanel } from './firmware';
import {
  AlertTriangle,
  Antenna,
  Cable,
  Check,
  CheckCircle,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  ChevronUp,
  CircleGauge,
  Clock,
  Cpu,
  Crosshair,
  Layers,
  RefreshCw,
  ScrollText,
  Send,
  SlidersHorizontal,
  Wrench,
  type IconComponent,
} from './icons';
import { formatUptime } from './math';
import {
  api,
  latestRow,
  postCommand,
  useBool,
  useNumber,
  useRing,
  useSelection,
  useText,
  useVec2,
} from './model';

// ------------------------------------------------------------------ installation scope

function InstallationInspector() {
  return (
    <div className="stack">
      <Panel title={<><Layers />Arrangement</>}>
        <Row label="Columns">
          <NumberField path="/installation/arrangement/columns" />
        </Row>
        <Row label="Rows">
          <NumberField path="/installation/arrangement/rows" />
        </Row>
        <Row label="Column width">
          <NumberField path="/installation/arrangement/column_width" />
        </Row>
        <Row label="Flipped">
          <Toggle path="/installation/arrangement/flipped" />
        </Row>
        <Action path="/installation/actions/rebuild_columns">Rebuild columns</Action>
      </Panel>
      <Panel title={<><Send />Messaging</>}>
        <Row label="Transmit">
          <EnumSelect path="/installation/messaging/transmit" />
        </Row>
        <Row label="Period">
          <NumberField path="/installation/messaging/period_s" />
        </Row>
        <Row label="Keyframe batch">
          <NumberField path="/installation/messaging/keyframe_batch" />
        </Row>
        <Row label="Keyframe velocities">
          <Toggle path="/installation/messaging/keyframe_velocities" />
        </Row>
        <Row label="Image sampling">
          <Toggle path="/installation/image_enabled" />
        </Row>
      </Panel>
      <Panel title={<><Antenna />All portals</>}>
        <Row label="Max velocity">
          <NumberField path="/bulk/max_velocity" />
        </Row>
        <Row label="Acceleration">
          <NumberField path="/bulk/acceleration" />
        </Row>
        <Action path="/bulk/actions/push_motion_profile">Push motion profile</Action>
        <Row label="Current">
          <NumberField path="/bulk/current_amps" />
        </Row>
        <Action path="/bulk/actions/set_current">Set current on all</Action>
      </Panel>
      <FirmwarePanel col={null} />
    </div>
  );
}

// ------------------------------------------------------------------ column scope

interface PortsDoc {
  ports: string[];
}

function DevicePicker({ col }: { col: number }) {
  const device = useParam<string>(`/columns/${col}/rs485/device`);
  const [ports, setPorts] = useState<string[]>([]);
  const [custom, setCustom] = useState('');
  const refresh = () => {
    api<PortsDoc>('/api/router/ports')
      .then((doc) => setPorts(doc.ports))
      .catch(() => setPorts([]));
  };
  useEffect(refresh, []);
  const choose = (settings: Record<string, unknown>) => device.set(JSON.stringify(settings));
  const current = device.value ?? '';
  const rows: { title: string; detail: string; settings: Record<string, unknown> }[] = [
    ...ports.map((port) => ({
      title: port,
      detail: 'serial · 115200 8N1',
      settings: { deviceType: 'Serial', address: port },
    })),
    // The two gateway presets the C++ device picker always offered.
    ...['192.168.1.201', '192.168.1.202'].map((host) => ({
      title: host,
      detail: 'RS485-over-TCP gateway · port 4196',
      settings: { deviceType: 'TCP', address: host, port: 4196 },
    })),
  ];
  return (
    <div className="device-picker">
      {rows.map((row) => {
        const json = JSON.stringify(row.settings);
        const selected =
          current.includes(`"${row.settings.address}"`) &&
          current.includes(`"${row.settings.deviceType}"`);
        return (
          <button
            key={json}
            type="button"
            className="choice-row"
            data-selected={selected}
            onClick={() => choose(row.settings)}
          >
            <span className="choice-mark">{selected && <Check />}</span>
            <span className="choice-copy">
              <strong>{row.title}</strong>
              <small>{row.detail}</small>
            </span>
          </button>
        );
      })}
      <div className="row wrap">
        <input
          className="custom-device"
          placeholder="custom host or port path…"
          value={custom}
          onChange={(event) => setCustom(event.target.value)}
          aria-label="Custom device address"
        />
        <Button
          variant="quiet"
          disabled={!custom}
          onClick={() =>
            choose(
              custom.includes('.') || custom.includes(':')
                ? { deviceType: 'TCP', address: custom.split(':')[0], port: Number(custom.split(':')[1] ?? 4196) }
                : { deviceType: 'Serial', address: custom },
            )
          }
        >
          <Cable />
          Use custom
        </Button>
        <Button variant="quiet" onClick={refresh}>
          <RefreshCw />
          Refresh ports
        </Button>
      </div>
    </div>
  );
}

function ColumnInspector({ col }: { col: number }) {
  const connected = useBool(`/columns/${col}/rs485/connected`);
  const description = useText(`/columns/${col}/rs485/device_description`);
  const tx = useNumber(`/columns/${col}/rs485/tx_count`);
  const rx = useNumber(`/columns/${col}/rs485/rx_count`);
  const timeouts = useNumber(`/columns/${col}/rs485/ack_timeouts`);
  const errors = useNumber(`/columns/${col}/rs485/decode_errors`);
  const [countX, countY] = useVec2(`/columns/${col}/shape`);
  return (
    <div className="stack">
      <Panel
        title={<><Cable />{`Column ${col + 1} · RS485`}</>}
        right={<Badge tone={connected ? 'ok' : 'error'}>{connected ? 'connected' : 'down'}</Badge>}
      >
        {connected ? (
          <>
            <Fact label="Device" value={description || '—'} />
            <Action path={`/columns/${col}/actions/disconnect`}>Disconnect</Action>
          </>
        ) : (
          <>
            <DevicePicker col={col} />
            <Action path={`/columns/${col}/actions/connect`} variant="primary">
              Connect
            </Action>
          </>
        )}
        <div className="fact-grid">
          <Fact label="Tx" value={tx} />
          <Fact label="Rx" value={rx} />
          <Fact label="ACK timeouts" value={timeouts} tone={timeouts > 0 ? 'warn' : undefined} />
          <Fact label="Decode errors" value={errors} tone={errors > 0 ? 'warn' : undefined} />
        </div>
        <div className="row wrap">
          <Action path={`/columns/${col}/actions/clear_outbox`}>Clear outbox</Action>
          <Action path={`/columns/${col}/actions/clear_counters`}>Clear counters</Action>
        </div>
      </Panel>
      <Panel title={<><Clock />Scheduled poll</>}>
        <Row label="Enabled">
          <Toggle path={`/columns/${col}/scheduled_poll/enabled`} />
        </Row>
        <Row label="Period">
          <NumberField path={`/columns/${col}/scheduled_poll/period_s`} />
        </Row>
      </Panel>
      <Panel title={<><Antenna />{`Broadcast to column ${col + 1}`}</>}>
        <BroadcastActions prefix={`/columns/${col}`} />
      </Panel>
      <Panel title={<><Crosshair />Pilot all (column)</>}>
        <PilotAllPad path={`/columns/${col}/pilot_all`} size={120} />
      </Panel>
      <FirmwarePanel col={col} />
      <Fact label="Portals" value={`${countX} × ${countY}`} />
    </div>
  );
}

// ------------------------------------------------------------------ portal scope

function PilotSub() {
  return (
    <div className="stack">
      <div className="pilot-disk-row">
        <PilotDisk size={280} />
        <div className="pilot-numbers">
          <Row label="Leading">
            <EnumSelect path="/portal/pilot/leading" />
          </Row>
          <Row label="x">
            <NumberField path="/portal/pilot/position" lane={0} />
          </Row>
          <Row label="y">
            <NumberField path="/portal/pilot/position" lane={1} />
          </Row>
          <Row label="r">
            <NumberField path="/portal/pilot/polar" lane={0} />
          </Row>
          <Row label="θ">
            <NumberField path="/portal/pilot/polar" lane={1} />
          </Row>
          <Row label="Offset">
            <NumberField path="/portal/pilot/offset" />
          </Row>
        </div>
      </div>
      <div className="dial-row">
        {[0, 1].map((axis) => (
          <div key={axis} className="dial-block">
            <AxisDial axis={axis as 0 | 1} size={140} />
            <div className="dial-quick">
              {/* Each direction gets its own chevron. The iced GUI drew the *right* chevron
                  on all four, which made the pad useless as a shape and readable only by
                  its words; the openFrameworks app had four, and so does this. */}
              {([
                ['Left', 0, ChevronLeft],
                ['Up', 0.25, ChevronUp],
                ['Right', 0.5, ChevronRight],
                ['Down', 0.75, ChevronDown],
              ] as const).map(([label, value, glyph]) => (
                <QuickAxisButton key={label} axis={axis as 0 | 1} value={value} icon={glyph}>
                  {label}
                </QuickAxisButton>
              ))}
            </div>
            <Row label={axis === 0 ? 'a' : 'b'}>
              <NumberField path="/portal/pilot/axes" lane={axis} />
            </Row>
          </div>
        ))}
      </div>
      <div className="row wrap">
        <Action path="/portal/actions/reset_local">Reset local (r)</Action>
        <Action path="/portal/actions/unwind">Unwind (u)</Action>
        <Action path="/portal/actions/push">Push (m)</Action>
        <Action path="/portal/actions/poll_position">Poll position</Action>
        <Action path="/portal/actions/take_current">Take current</Action>
        <Action path="/portal/actions/see_through_local">See through (local)</Action>
      </div>
      <Row label="Send periodically">
        <Toggle path="/portal/pilot/send_periodically" />
      </Row>
    </div>
  );
}

function QuickAxisButton({
  axis,
  value,
  children,
  icon: Glyph,
}: {
  axis: 0 | 1;
  value: number;
  children: React.ReactNode;
  icon: IconComponent;
}) {
  const axes = useParam<number[]>('/portal/pilot/axes');
  return (
    <Button
      variant="quiet"
      onClick={() => {
        const current = Array.isArray(axes.value) ? axes.value : [0, 0];
        const next: [number, number] =
          axis === 0 ? [value, current[1] ?? 0] : [current[0] ?? 0, value];
        axes.set(next);
      }}
    >
      <Glyph />
      {children}
    </Button>
  );
}

function AxisSub({ axis }: { axis: 0 | 1 }) {
  const name = axis === 0 ? 'a' : 'b';
  const position = useNumber(`/portal/axis/${name}/reported_position`);
  const target = useNumber(`/portal/axis/${name}/reported_target`);
  const health = useNumber(`/portal/axis/${name}/health_ok`);
  return (
    <div className="stack">
      <div className="fact-grid">
        <Fact label="Position" value={position} />
        <Fact label="Target" value={target} />
        {/* Calibration is the one fact here that is a verdict rather than a number, so it
            carries the verdict's glyph. `unknown` gets neither: nothing has been measured. */}
        <Fact
          label="Calibration"
          value={
            <>
              {health === 1 ? <CheckCircle /> : health === 0 ? <AlertTriangle /> : null}
              {health === 1 ? 'ok' : health === 0 ? 'fault' : 'unknown'}
            </>
          }
          tone={health === 0 ? 'error' : undefined}
        />
      </div>
      <Panel title={<><CircleGauge />Motion profile</>}>
        <Row label="Max velocity">
          <NumberField path={`/portal/axis/${name}/profile/max_velocity`} />
        </Row>
        <Row label="Acceleration">
          <NumberField path={`/portal/axis/${name}/profile/acceleration`} />
        </Row>
        <Row label="Min velocity">
          <NumberField path={`/portal/axis/${name}/profile/min_velocity`} />
        </Row>
        <Action path={`/portal/axis/${name}/actions/push_profile`}>Push motion profile</Action>
      </Panel>
      <Panel title={<><Wrench />Routines</>}>
        <div className="row wrap">
          <Action path={`/portal/axis/${name}/actions/zero_position`}>Zero position</Action>
          <Action path={`/portal/axis/${name}/actions/measure_backlash`}>Measure backlash</Action>
          <Action path={`/portal/axis/${name}/actions/home_routine`}>Home routine</Action>
          <Action path={`/portal/axis/${name}/actions/init_timer`}>Init timer</Action>
          <Action path={`/portal/axis/${name}/actions/deinit_timer`}>Deinit timer</Action>
          <Action path={`/portal/axis/${name}/actions/test_timer`}>Test timer</Action>
          <Action path={`/portal/axis/${name}/actions/md_test_routine`}>MD test routine</Action>
          <Action path={`/portal/axis/${name}/actions/md_test_timer`}>MD test timer</Action>
        </div>
      </Panel>
    </div>
  );
}

function MotorSub() {
  return (
    <div className="stack">
      <Panel title={<><SlidersHorizontal />Motor driver settings</>}>
        <Row label="Current">
          <NumberField path="/portal/mds/current_amps" />
        </Row>
        <Row label="Microsteps" hint="transmitted to hardware as log2">
          <EnumSelect path="/portal/mds/microstep_resolution" />
        </Row>
      </Panel>
    </div>
  );
}

interface LogsDoc {
  logs: { level: number; message: string; count: number }[];
}

function LogSub({ col, portal }: { col: number; portal: number }) {
  const [logs, setLogs] = useState<LogsDoc['logs']>([]);
  useEffect(() => {
    let alive = true;
    const poll = () => {
      api<LogsDoc>(`/api/router/logs?col=${col}&portal=${portal}`)
        .then((doc) => {
          if (alive) setLogs(doc.logs);
        })
        .catch(() => {});
    };
    poll();
    const timer = setInterval(poll, 500);
    return () => {
      alive = false;
      clearInterval(timer);
    };
  }, [col, portal]);
  const level = (l: number) => (l >= 20 ? 'error' : l >= 10 ? 'warn' : 'ok');
  return (
    <div className="log-viewer">
      <div className="row wrap">
        <Action path="/portal/log/actions/clear" variant="quiet">
          Clear
        </Action>
      </div>
      {logs.length === 0 && <p className="placeholder">No firmware log lines yet.</p>}
      {[...logs].reverse().map((line, i) => (
        <div key={`${i}-${line.message}`} className={`log-line is-${level(line.level)}`}>
          <span className="log-dot" />
          <span className="log-text">{line.message}</span>
          {line.count > 1 && <span className="log-count">×{line.count}</span>}
        </div>
      ))}
    </div>
  );
}

/** The C++ portal header's "time since last message" history, from the rx-age lane. */
function MsgAgeSparkline() {
  const { ringIndex } = useTelemetry('/tel/portal/selected');
  if (ringIndex < 0) return null;
  return (
    <span className="msg-age-spark" title="Time since last message">
      <Sparkline channel={ringIndex} lane={SEL.rxAge} height={24} />
    </span>
  );
}

type PortalSub = 'pilot' | 'axis_a' | 'axis_b' | 'motor' | 'log';

function PortalInspector({ col, portal }: { col: number; portal: number }) {
  const exists = useBool('/portal/exists');
  const uptime = useNumber('/portal/state/uptime_ms');
  const version = useText('/portal/state/version');
  const inPosition = useBool('/portal/state/in_position');
  const lastLog = useText('/portal/state/last_log');
  const lastLogLevel = useNumber('/portal/state/last_log_level');
  const ring = useRing('/tel/portal/selected');
  const [sub, setSub] = useState<PortalSub>('pilot');
  const row = latestRow(ring);
  const rxFresh = row ? row[SEL.rxAge] < 200 : false;

  if (!exists) {
    return <p className="placeholder">Portal {portal} not found in column {col + 1}.</p>;
  }
  return (
    <div className="stack" data-av-surface="portal-pilot">
      <div className="portal-header">
        <span className="portal-id">#{portal}</span>
        <Badge tone={rxFresh ? 'ok' : 'idle'}>{rxFresh ? 'live' : 'quiet'}</Badge>
        <Badge tone={inPosition ? 'ok' : 'idle'}>{inPosition ? 'in position' : 'moving'}</Badge>
        <MsgAgeSparkline />
      </div>
      <div className="fact-grid">
        <Fact label="Uptime" value={formatUptime(uptime)} />
        <Fact label="Firmware" value={version || '—'} />
      </div>
      {lastLog && (
        <div className={`last-log is-${lastLogLevel >= 20 ? 'error' : lastLogLevel >= 10 ? 'warn' : 'ok'}`}>
          {lastLog}
        </div>
      )}
      <Panel title={<><Antenna />Actions</>}>
        <BroadcastActions prefix="/portal" />
      </Panel>
      <Panel title={<><Clock />Polling</>}>
        <Row label="Poll regularly">
          <Toggle path="/portal/poll/regularly" />
        </Row>
        <Row label="Interval">
          <NumberField path="/portal/poll/interval_s" />
        </Row>
      </Panel>
      <Tabs
        value={sub}
        onChange={setSub}
        label="Portal sub-panels"
        items={[
          { id: 'pilot', label: <><Crosshair />Pilot</> },
          { id: 'axis_a', label: <><CircleGauge />Axis A</> },
          { id: 'axis_b', label: <><CircleGauge />Axis B</> },
          { id: 'motor', label: <><Cpu />Motor</> },
          { id: 'log', label: <><ScrollText />Log</> },
        ]}
      />
      {sub === 'pilot' && <PilotSub />}
      {sub === 'axis_a' && <AxisSub axis={0} />}
      {sub === 'axis_b' && <AxisSub axis={1} />}
      {sub === 'motor' && <MotorSub />}
      {sub === 'log' && <LogSub col={col} portal={portal} />}
    </div>
  );
}

// ------------------------------------------------------------------ the inspector shell

export function Inspector() {
  const selection = useSelection();
  return (
    <aside className="inspector">
      <nav className="breadcrumb" aria-label="Selection">
        <button type="button" onClick={selection.selectInstallation}>
          Installation
        </button>
        {(selection.kind === 'column' || selection.kind === 'portal') && (
          <>
            <ChevronRight />
            <button type="button" onClick={() => selection.selectColumn(selection.col)}>
              Column {selection.col + 1}
            </button>
          </>
        )}
        {selection.kind === 'portal' && (
          <>
            <ChevronRight />
            <span className="crumb-current">Portal {selection.portal}</span>
          </>
        )}
      </nav>
      {selection.kind === 'portal' ? (
        <PortalInspector col={selection.col} portal={selection.portal} />
      ) : selection.kind === 'column' ? (
        <ColumnInspector col={selection.col} />
      ) : (
        <InstallationInspector />
      )}
    </aside>
  );
}

/** Keyboard shortcuts r/u/m on the selected portal, suppressed inside inputs. */
export function useKeyboardShortcuts() {
  const reset = useParam<number>('/portal/actions/reset_local');
  const unwind = useParam<number>('/portal/actions/unwind');
  const push = useParam<number>('/portal/actions/push');
  const selection = useSelection();
  useEffect(() => {
    const bump = (p: { value: number | undefined; set: (v: number) => void }) =>
      p.set((p.value ?? 0) + 1);
    const onKey = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      if (target && ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName)) return;
      if (selection.kind !== 'portal') return;
      if (event.key === 'r') bump(reset);
      if (event.key === 'u') bump(unwind);
      if (event.key === 'm') bump(push);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });
}

// Re-exported so app.tsx can offer "poll via API" style affordances later.
export { postCommand };
