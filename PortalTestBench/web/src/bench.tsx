/**
 * The bench page.
 *
 * Four bands, top to bottom, in the order an operator asks the questions:
 *
 *   1. verdict  — what is the answer, or what is happening right now
 *   2. live     — what is the module doing this instant
 *   3. control  — what can I do to it
 *   4. evidence — what did it tell me, and how is this bench configured
 *
 * The verdict band never scrolls away and is never blank: `verdictTile` in `bench-model.ts`
 * decides its content, and every branch of it is unit-tested. This file should read as layout.
 */

import {
  Badge,
  Banner,
  Button,
  EnumSelect,
  Panel,
  ParamTree,
  Row,
  StatusBar,
  StatusItem,
  TextField,
  TitleBar,
} from '@auroravision/av-gui/controls';
import { mount, useParam, useSchema } from '@auroravision/av-gui/runtime';
import '@auroravision/av-gui/styles.css';
import { useEffect, type ReactNode } from 'react';
import { SessionLog } from './bench-log';
import { type BenchView, thresholdTone, verdictTile, whyDisabled } from './bench-model';

/* ---------------------------------------------------------------- small bindings ------- */

/** Read a read-only enum back **by name**. Never key a page on a discriminant. */
function useEnumName(path: string): string {
  const p = useParam<number>(path);
  const variant = p.decl?.variants.find((v) => v.value === p.value);
  return variant?.name ?? 'unknown';
}

function useText(path: string): string {
  return useParam<string>(path).value ?? '';
}

function useNumber(path: string): number {
  return useParam<number>(path).value ?? 0;
}

function useBool(path: string): boolean {
  return !!useParam<boolean>(path).value;
}

/**
 * An action. Bumps a counter rather than setting a flag, so pressing it twice does the thing
 * twice and a page that reconnects mid-run does not re-trigger whatever it finds set.
 *
 * `why` is both the disable condition and the tooltip: a control that is greyed out without
 * saying why is a control someone has to come and ask about.
 */
function Action({
  path,
  children,
  why,
  variant,
}: {
  path: string;
  children: ReactNode;
  why: string | null;
  variant?: 'default' | 'primary' | 'danger' | 'quiet';
}) {
  const p = useParam<number>(path);
  const disabled = why !== null || !p.decl;
  return (
    <Button
      variant={variant}
      disabled={disabled}
      title={why ?? p.decl?.label ?? path}
      onClick={() => p.set((p.value ?? 0) + 1)}
    >
      {children}
    </Button>
  );
}

/** A value with its own tone. Used wherever "not measured" must not look like zero. */
function Fact({ label, value, tone }: { label: string; value: ReactNode; tone?: string }) {
  return (
    <div className={`fact${tone ? ` is-${tone}` : ''}`}>
      <span className="fact-label">{label}</span>
      <span className="fact-value">{value}</span>
    </div>
  );
}

/**
 * A health flag, with three states rather than two.
 *
 * "Nobody has asked" is not "asked and the answer was no". Drawing an unread flag as a failure
 * sends someone looking for a fault that has not been measured; drawing it as a pass is worse.
 * `known` therefore means a module has answered, not that the parameter is declared.
 */
function Flag({ path, label, known }: { path: string; label: string; known: boolean }) {
  const p = useParam<boolean>(path);
  const state = !known ? ' is-unknown' : p.value ? ' is-ok' : ' is-bad';
  const title = !known ? `${label}: not measured` : `${label}: ${p.value ? 'ok' : 'not ok'}`;
  return (
    <span className={`flag${state}`} title={title}>
      {label}
    </span>
  );
}

/* ---------------------------------------------------------------- the bands ------------ */

function VerdictBand({ view }: { view: BenchView }) {
  const tile = verdictTile(view);
  return (
    <section className="band band-verdict" data-av-surface="test-runner" data-tone={tile.tone}>
      <div className="verdict">
        <span className="verdict-word">{tile.word}</span>
        <span className="verdict-detail">{tile.detail}</span>
      </div>
      <div className="verdict-actions">
        {/* Whoever is at the screen must never be surprised about who is driving the hardware
            in front of them. An agent and a human share one command queue by design. */}
        {view.runBusy && (view.runOrigin === 'agent' || view.runOrigin === 'cli') && (
          <Badge tone="active" title={`this run was started by ${view.runOrigin}, not from this page`}>
            {view.runOrigin} driving
          </Badge>
        )}
        {/* Editable while idle, and deliberately still visible while a run is in flight so the
            operator can see which plan the "Run" button would start next. */}
        <div className="plan-picker">
          <TextField path="/plan/selected" />
        </div>
        <Action path="/actions/run" why={whyDisabled(view, { destructive: true }, false)} variant="primary">
          Run plan
        </Action>
        <Action path="/actions/abort" why={view.runBusy ? null : 'nothing is running'} variant="danger">
          Abort
        </Action>
        <Action path="/actions/escape" why={whyDisabled(view, {}, false)}>
          Escape routine
        </Action>
      </div>
    </section>
  );
}

/**
 * What the module is doing this instant.
 *
 * Position and target are drawn as two separate marks, never one: a commanded position and a
 * real one disagreeing is the single most informative thing on this page. The threshold triple
 * sits here too, because it is the number most likely to be quietly wrong.
 */
function LiveBand() {
  const threshold = {
    floor: useNumber('/dut/threshold/floor'),
    band: useNumber('/dut/threshold/band'),
    applied: useNumber('/dut/threshold/applied'),
    calibratedAtS: useNumber('/dut/threshold/calibrated_at_s'),
  };
  const t = thresholdTone(threshold);
  return (
    <section className="band band-live" data-av-surface="live-telemetry">
      <Panel title="Motion">
        <div className="axes">
          <AxisLive axis="a" />
          <AxisLive axis="b" />
        </div>
      </Panel>
      <Panel title="Optical home threshold">
        <div className={`threshold is-${t.tone}`}>
          <Fact label="floor" value={threshold.calibratedAtS < 0 ? '—' : threshold.floor} />
          <Fact label="band" value={threshold.calibratedAtS < 0 ? '—' : `${threshold.band}`} />
          <Fact label="applied" value={threshold.calibratedAtS < 0 ? '—' : threshold.applied} />
          {t.tone === 'ok' ? (
            <p className="threshold-note">{t.text}</p>
          ) : (
            <Banner tone={t.tone === 'error' ? 'error' : 'warn'} role="status">
              {t.text}
            </Banner>
          )}
        </div>
      </Panel>
    </section>
  );
}

/**
 * Whether a value has actually been *reported* — not whether the parameter exists, and not
 * whether a module is attached.
 *
 * Three different questions, and conflating them fabricates measurements. The declaration is
 * always present; a module can be plainly connected and still never report a position (the
 * production firmware's serial menu never does — positions live on the RS485 side). Drawing a
 * confident `0` for an axis nobody has measured is worse than a blank, because it looks like
 * data. So the worker publishes an explicit `position_known` / `health_known` per axis, and
 * this is the only thing the page trusts.
 */
function AxisLive({ axis }: { axis: 'a' | 'b' }) {
  const position = useParam<number>(`/dut/${axis}/position`);
  const target = useParam<number>(`/dut/${axis}/target`);
  const known = useBool(`/dut/${axis}/position_known`);
  const healthKnown = useBool(`/dut/${axis}/health_known`);
  const error = (target.value ?? 0) - (position.value ?? 0);
  return (
    <div className="axis">
      <span className="axis-name">{axis.toUpperCase()}</span>
      <Fact label="position" value={known ? position.value : '—'} />
      <Fact label="target" value={known ? target.value : '—'} />
      <Fact label="error" value={known ? error : '—'} tone={known && error !== 0 ? 'warn' : undefined} />
      <div className="axis-flags">
        <Flag path={`/dut/${axis}/health_home`} label="home" known={healthKnown} />
        <Flag path={`/dut/${axis}/health_switches`} label="switches" known={healthKnown} />
        <Flag path={`/dut/${axis}/health_backlash`} label="backlash" known={healthKnown} />
        <Flag path={`/dut/${axis}/health_measure_cycle`} label="cycle" known={healthKnown} />
      </div>
    </div>
  );
}

function ControlBand({ view }: { view: BenchView }) {
  return (
    <section className="band band-control">
      <Panel title="Transport" scope="/transport">
        <div data-av-surface="transport">
          <Row label="Link">
            <EnumSelect path="/transport/desired" />
          </Row>
          <Row label="Port" hint="COM7, or 192.168.0.50:4196 for a gateway">
            <TextField path="/transport/port" />
          </Row>
          <div className="button-row">
            <Action path="/actions/connect" why={view.connected ? 'already connected' : null}>
              Connect
            </Action>
            <Action path="/actions/disconnect" why={view.connected ? null : 'not connected'}>
              Disconnect
            </Action>
            <Action path="/actions/rescan" why={null}>
              Rescan
            </Action>
            <Action path="/actions/identify" why={whyDisabled(view, {}, false)}>
              Identify
            </Action>
          </div>
        </div>
      </Panel>

      <Panel title="Operations">
        <div className="button-row">
          <Action
            path="/actions/calibrate_threshold"
            why={whyDisabled(view, { needsFirmware: 'production', destructive: true }, false)}
          >
            Calibrate threshold
          </Action>
          <Action path="/actions/home_a" why={whyDisabled(view, { destructive: true }, false)}>
            Home A
          </Action>
          <Action path="/actions/home_b" why={whyDisabled(view, { destructive: true }, false)}>
            Home B
          </Action>
          <Action path="/actions/unjam_a" why={whyDisabled(view, { destructive: true }, false)}>
            Unjam A
          </Action>
          <Action path="/actions/unjam_b" why={whyDisabled(view, { destructive: true }, false)}>
            Unjam B
          </Action>
          <Action path="/actions/marker" why={null}>
            Drop marker
          </Action>
        </div>
        <Banner tone="info">
          Unjamming loses the datum: the axis must be homed again before any position it reports
          means anything.
        </Banner>
      </Panel>

      <Panel title="Firmware">
        <div data-av-surface="firmware">
          <Row label="On the module">
            <Badge tone={view.firmwareKind === 'unknown' ? 'offline' : 'active'}>{view.firmwareKind}</Badge>
          </Row>
          <div className="button-row">
            <Action path="/actions/flash" why={whyDisabled(view, { needsModule: false, destructive: true }, false)}>
              Flash
            </Action>
          </div>
          <Banner tone="warn" role="alert">
            The bench image is a flat build at 0x08000000: flashing it <strong>erases the RS485
            bootloader</strong>, and the module cannot be field-updated again until a production
            image is written back over SWD.
          </Banner>
        </div>
      </Panel>
    </section>
  );
}

function EvidenceBand({ view }: { view: BenchView }) {
  const version = useText('/dut/version');
  const uptime = useNumber('/dut/uptime_s');
  const ratio = useEnumName('/dut/ratio');
  const ustepsPerRev = useNumber('/dut/usteps_per_rev');
  const reason = useText('/last/reason');
  const advice = useText('/last/advice');
  const measurements = useText('/last/measurements');
  const reportPath = useText('/last/report_path');
  const faults = useNumber('/faults/active');

  return (
    <section className="band band-evidence">
      <Panel title="Module">
        <div data-av-surface="module-state">
          <Fact label="firmware" value={view.firmwareKind} />
          <Fact label="version" value={version || '—'} />
          <Fact label="uptime" value={view.modulePresent ? `${uptime} s` : '—'} />
          <Fact label="gearing" value={ratio === 'unknown' ? '—' : `${ratio}:1`} />
          <Fact label="µsteps / rev" value={ustepsPerRev > 0 ? ustepsPerRev : '—'} />
        </div>
      </Panel>

      <Panel title="Last result">
        <div className="last-result">
          <Fact label="plan" value={view.lastPlan || '—'} />
          <Fact label="verdict" value={view.lastVerdict} />
          <Fact label="measurements" value={measurements || '—'} />
          {reason && <p className="reason">{reason}</p>}
          {advice && <p className="advice">{advice}</p>}
            <Fact label="report" value={reportPath || '—'} />
          <div data-av-surface="faults" className="faults">
            <Fact label="active faults" value={faults} tone={faults > 0 ? 'error' : undefined} />
          </div>
        </div>
      </Panel>

      <Panel title="How this bench is set up">
        {/* Read-only, and on screen rather than in a dialog: the first question about a
            surprising measurement is always "what was it set to". */}
        <ParamTree prefix="/setup" />
      </Panel>
    </section>
  );
}

/* ---------------------------------------------------------------- the app -------------- */

/**
 * Tell the bench this page is alive.
 *
 * Stays here in `App`, above every band, deliberately: moving it into a panel that some layout
 * change later unmounts would silently stop the beat, and the bench would start refusing
 * destructive operations for no visible reason. See `whyDisabled` for the narrow thing
 * staleness is allowed to do — it never cancels a run in flight.
 */
function useHeartbeat() {
  const { set } = useParam<number>('/ui/heartbeat');
  useEffect(() => {
    const id = setInterval(() => set(Date.now()), 1000);
    return () => clearInterval(id);
  }, [set]);
}

function App() {
  const schema = useSchema();
  useHeartbeat();

  const view: BenchView = {
    connected: useBool('/transport/connected'),
    transportObserved: useEnumName('/transport/observed'),
    modulePresent: useBool('/dut/present'),
    firmwareKind: useEnumName('/dut/firmware_kind'),
    runBusy: useBool('/run/busy'),
    runPlan: useText('/run/plan'),
    runPhase: useEnumName('/run/phase'),
    runOrigin: useEnumName('/run/origin'),
    stepName: useText('/run/step_name'),
    stepIndex: useNumber('/run/step_index'),
    stepCount: useNumber('/run/step_count'),
    cycle: useNumber('/run/cycle'),
    cycleCount: useNumber('/run/cycle_count'),
    lastVerdict: useEnumName('/last/verdict'),
    lastPlan: useText('/last/plan'),
    lastReason: useText('/last/reason'),
  };

  const passed = useNumber('/counts/passed');
  const failed = useNumber('/counts/failed');
  const faults = useNumber('/faults/active');
  const port = useNumber('/setup/http_port');
  const simulated = useBool('/setup/simulated');

  return (
    /*
     * `app app--filled` is the framework's own root, and both halves matter.
     *
     * `.app` is the three-row grid — title bar, content, status bar — where only the middle row
     * scrolls. `.app--filled` opts back in to an opaque background: the framework deliberately
     * leaves the document TRANSPARENT so a Vulkan compositor can draw underneath a page, and a
     * screen that renders that hole with nothing behind it just shows through to whatever the
     * webview paints, which is white. This app is a `control-window` with no viewport and no
     * compositor, so it owns every pixel and must say so.
     */
    <div className="app app--filled bench" data-connected={view.connected} data-run-phase={view.runPhase}>
      {/* Not decoration: under a native window the title bar's geometry is a hit-test contract
          with the shell, so a hand-rolled header gives window buttons that light up and do
          nothing. */}
      <TitleBar
        title="Portal Test Bench"
        sub={schema === null ? 'connecting' : simulated ? 'simulated module' : 'one module under test'}
      />

      <div className="bench-main">
        <div className="stack bench-stack">
          <VerdictBand view={view} />
          <LiveBand />
          <ControlBand view={view} />
          <EvidenceBand view={view} />
        </div>
        {/* Pinned: the module's output is the thing you watch while a routine runs, and a log
            that scrolls away with the page is a log you have to go looking for. */}
        <SessionLog />
      </div>

      <StatusBar stream={null}>
        <StatusItem label="link" value={view.connected ? view.transportObserved : 'down'} tone={view.connected ? 'ok' : 'error'} />
        <StatusItem label="module" value={view.modulePresent ? view.firmwareKind : 'none'} tone={view.modulePresent ? 'ok' : 'warn'} />
        <StatusItem label="runs" value={`${passed} pass · ${failed} fail`} />
        {/* The contract requires the active fault count to ride here in an error tone. */}
        {faults > 0 && <StatusItem label="faults" value={String(faults)} tone="error" />}
        <StatusItem label="port" value={port > 0 ? String(port) : '—'} />
      </StatusBar>
    </div>
  );
}

mount(<App />);
