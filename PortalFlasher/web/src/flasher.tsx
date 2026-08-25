/**
 * The rig's page.
 *
 * Three bands. The **console** across the top carries the verdict and the two controls that act on
 * it; **probe** and **firmware** are the setup underneath; the **evidence** — what is on the board,
 * what has happened this session — is below that. An operator working properly is not looking at
 * any of it — they are listening — so the screen's job is to answer the question they have *after*
 * a tone surprised them, and to make the setup obvious before they start.
 *
 * Two things here are safety mechanisms rather than decoration:
 *
 * - **The heartbeat.** This page re-asserts `/ui/heartbeat` once a second and the worker drops
 *   out of auto-flash if it goes stale. Sound lives in the browser, so a rig nobody can hear must
 *   not stay armed; closing this tab disarms it within three seconds.
 * - **The mode cue.** Switching to auto-flash plays a tone. It satisfies the browser's autoplay
 *   gesture requirement on a real click, *and* it is the operator's only chance to notice that
 *   audio is muted before a board is flashed in silence — `SystemSounds` swallows play()
 *   rejections by design.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import { SystemSounds } from '@auroravision/av-gui/calibration';
import {
  Badge,
  Banner,
  Button,
  EmptyState,
  Panel,
  ParamFilter,
  ParamTree,
  Row,
  Section,
  StatusBar,
  StatusItem,
  Tabs,
  TitleBar,
  Toggle,
} from '@auroravision/av-gui/controls';
import { mount, useParam, useSchema } from '@auroravision/av-gui/runtime';
import '@auroravision/av-gui/styles.css';

import { FirmwareMap } from './FirmwareMap';
import { type MapModel, buildMap, comparisonSummary, formatBytes } from './firmware-map';
import {
  type Cue,
  type Expect,
  type LastPass,
  type Layout,
  type Mode,
  type Outcome,
  type Phase,
  type RigState,
  type Step,
  flashNowState,
  lastPassSummary,
  layoutSummary,
  readDeviceState,
  shortHash,
  soundFor,
  statusSummary,
  stepSummary,
  tileFor,
} from './flasher-model';

const PROBE_SLOTS = 4;
const ARTEFACT_SLOTS = 6;

/** Read an enumeration by name. Never by discriminant — see `flasher-model.ts`. */
function useEnumName<T extends string>(path: string, fallback: T): T {
  const p = useParam<number>(path);
  const name = p.decl?.variants?.find((v) => v.value === Number(p.value ?? 0))?.name;
  return (name as T) ?? fallback;
}

function useText(path: string): string {
  return String(useParam<string>(path).value ?? '');
}

function useNumber(path: string): number {
  return Number(useParam<number>(path).value ?? 0);
}

function useFlag(path: string): boolean {
  return Boolean(useParam<boolean>(path).value);
}

/**
 * A one-shot action.
 *
 * Bumps a counter rather than toggling a flag, following `example-vision`: *"'try that again' is
 * a request and `Open` is a state."* A repeated press works, and a page that reconnects does not
 * re-trigger anything it sent before.
 */
function useAction(path: string): () => void {
  const p = useParam<number>(path);
  const value = Number(p.value ?? 0);
  const set = useRef(p.set);
  set.current = p.set;
  return useCallback(() => set.current(value + 1), [value]);
}

/** Keep the rig's dead-man fed for as long as this page is running. */
function useHeartbeat() {
  const beat = useParam<number>('/ui/heartbeat');
  const set = useRef(beat.set);
  set.current = beat.set;
  useEffect(() => {
    const id = window.setInterval(() => set.current(Date.now()), 1000);
    set.current(Date.now());
    return () => window.clearInterval(id);
  }, []);
}

/** Play a cue when, and only when, the sequence moves. */
function useCueSounds(sounds: SystemSounds, cue: Cue, seq: number) {
  const seen = useRef<number | null>(null);
  useEffect(() => {
    // A session that connects late adopts the current sequence without replaying its sound. The
    // rig's history is not something to be re-heard.
    if (seen.current === null) {
      seen.current = seq;
      return;
    }
    if (seq === seen.current) return;
    seen.current = seq;

    const action = soundFor(cue);
    if (action.kind === 'loop') {
      sounds.process('idle', `busy:${seq}`);
    } else if (action.kind === 'play') {
      sounds.stopIdle();
      sounds.play(action.name, `${action.name}:${seq}`);
    }
  }, [sounds, cue, seq]);
}

// ---------------------------------------------------------------- device document

interface RegionJson {
  name: string;
  base: string;
  used_bytes: number;
  sha256: string;
  banner: string | null;
  vector_ok: boolean;
  entry: string | null;
}

interface DeviceJson {
  read_at_ms: number;
  layout: string;
  field_updatable: boolean;
  uid: string;
  idcode: string | null;
  dev_id: string | null;
  flash_kb: number;
  programmed_bytes: number;
  total_bytes: number;
  regions: RegionJson[];
  flat_entry: string | null;
  options: {
    raw: string;
    rdp_level: number;
    iwdg_sw: boolean;
    nboot_sel: boolean;
    nrst_mode: number;
    warnings: string[];
  };
  occupancy: number[];
  selected_occupancy: number[] | null;
  sector_bytes: number;
  bootloader_sectors: number;
  flash_base: string;
}

/**
 * The device report is structured state, not a control surface, so it comes off a route rather
 * than the bus — the same line the framework's own router admin page draws. Polled at human rate,
 * well inside `constraints.md` §5, and only while something has been read.
 */
function useDeviceReport(readMarker: number): DeviceJson | null {
  const [report, setReport] = useState<DeviceJson | null>(null);
  useEffect(() => {
    let cancelled = false;
    const fetchOnce = async () => {
      try {
        const response = await fetch('/api/flasher/device', { cache: 'no-store' });
        if (cancelled) return;
        // 204 means nothing has been read yet, which is a different thing from a read that found
        // nothing — the page draws them differently.
        setReport(response.status === 204 ? null : ((await response.json()) as DeviceJson));
      } catch {
        /* the host is restarting; the next tick will pick it up */
      }
    };
    void fetchOnce();
    const id = window.setInterval(() => void fetchOnce(), 1000);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, [readMarker]);
  return report;
}

// ---------------------------------------------------------------- panels

/** One row of the probe list. A component per row, so every value it draws is a real
 * subscription — reading them imperatively meant the list never repainted when a name changed
 * with the count unchanged. */
function ProbeRow({ slot, selected }: { slot: number; selected: string }) {
  const pad = String(slot).padStart(2, '0');
  const id = useText(`/probe/${pad}/id`);
  const name = useText(`/probe/${pad}/name`);
  const serial = useText(`/probe/${pad}/serial`);
  const kind = useText(`/probe/${pad}/kind`);
  const selection = useParam<string>('/probe/selected');

  if (!id) return null;
  const on = id === selected;
  const meta = [kind, serial].filter(Boolean).join(' · ');

  return (
    <button
      type="button"
      role="option"
      aria-selected={on}
      className={`device-row${on ? ' is-selected' : ''}`}
      onClick={() => selection.set(id)}
    >
      <span className="device-name">{name || id}</span>
      <span className="device-sub label-caps">{meta}</span>
    </button>
  );
}

function ProbePanel({ connected }: { connected: boolean }) {
  const count = useNumber('/probe/count');
  const selected = useText('/probe/selected');
  const rescan = useAction('/actions/rescan');

  return (
    <Panel
      title="1 · Probe"
      right={
        <Badge tone={connected ? 'ok' : 'error'}>{connected ? 'connected' : 'not connected'}</Badge>
      }
    >
      {count === 0 ? (
        <EmptyState
          detail="No debug probes found. Plug in an ST-Link and rescan."
          action={<Button onClick={rescan}>Rescan</Button>}
        />
      ) : (
        <>
          <div className="device-list" role="listbox" aria-label="Debug probes">
            {Array.from({ length: PROBE_SLOTS }, (_, slot) => (
              <ProbeRow key={slot} slot={slot} selected={selected} />
            ))}
          </div>
          {count > 1 && !selected && (
            <Banner tone="warn">
              More than one probe is attached. Choose the one wired to the fixture — guessing is
              how the wrong board gets flashed.
            </Banner>
          )}
          <div className="device-tools">
            <Button onClick={rescan} variant="quiet">
              Rescan
            </Button>
          </div>
        </>
      )}
    </Panel>
  );
}

/**
 * One artefact the repository can flash.
 *
 * Clicking selects it for its own region, or clears it if it was already selected — which is how
 * the scope is chosen. There is no separate scope control: the scope *is* which regions were
 * picked, so the two can never disagree.
 */
function ArtefactRow({ slot }: { slot: number }) {
  const pad = String(slot).padStart(2, '0');
  const id = useText(`/image/${pad}/id`);
  const label = useText(`/image/${pad}/label`);
  const region = useText(`/image/${pad}/region`);
  const origin = useText(`/image/${pad}/origin`);
  const detail = useText(`/image/${pad}/detail`);
  const fits = useFlag(`/image/${pad}/fits`);

  const bootSelection = useParam<string>('/image/boot_id');
  const appSelection = useParam<string>('/image/app_id');

  if (!id) return null;
  const selection = region === 'bootloader' ? bootSelection : appSelection;
  const on = String(selection.value ?? '') === id;

  return (
    <button
      type="button"
      role="option"
      aria-selected={on}
      className={`device-row${on ? ' is-selected' : ''}${fits ? '' : ' is-faulted'}`}
      onClick={() => selection.set(on ? '' : id)}
      title={fits ? undefined : 'too large for its bank'}
    >
      <span className="device-name">{label}</span>
      <span className="device-sub label-caps">
        {[region, origin, detail].filter(Boolean).join(' · ')}
      </span>
    </button>
  );
}

function FirmwarePanel({ hasImage }: { hasImage: boolean }) {
  const count = useNumber('/image/count');
  const scope = useText('/image/scope');
  const source = useText('/image/source');
  const buildId = useText('/image/build_id');
  const bootSha = useText('/image/boot_sha');
  const appSha = useText('/image/app_sha');
  const hint = useText('/image/hint');
  const runCheck = useText('/image/run_check');

  return (
    <Panel
      title="2 · Firmware"
      right={hasImage ? <Badge tone={scope === 'full' ? 'ok' : 'idle'}>{scope}</Badge> : null}
    >
      {count === 0 ? (
        <EmptyState detail="Nothing to flash was found in the build tree." />
      ) : (
        <div className="device-list" role="listbox" aria-label="Firmware artefacts">
          {Array.from({ length: ARTEFACT_SLOTS }, (_, slot) => (
            <ArtefactRow key={slot} slot={slot} />
          ))}
        </div>
      )}

      {/* What is missing, and the command that produces it. A fresh clone has never built
          PortalFW, and an empty list would be true and useless. */}
      {hint && <Banner tone="info">{hint}</Banner>}

      {source === 'synthetic' && (
        <Banner tone="warn">A synthetic image. Nothing here corresponds to real firmware.</Banner>
      )}

      {hasImage && (
        <>
          <Row label="Build">{buildId || '—'}</Row>
          <Row label="Bootloader">{shortHash(bootSha)}</Row>
          <Row label="Application">{shortHash(appSha)}</Row>
          {/* Whether auto-flash can complete a pass with this image. Better learned here than
              from a run-check that fails on a board which is in fact perfectly fine. */}
          <Row label="Run-check" hint="How a flashed board is proved to be running">
            <span className="fw-hint">{runCheck || '—'}</span>
          </Row>
        </>
      )}
    </Panel>
  );
}

/**
 * How long the confirm stays armed before it forgets, in milliseconds.
 *
 * Long enough to read the label and move the mouse; short enough that a confirm left armed while
 * the operator turned away does not still fire when they come back and click something.
 */
const CONFIRM_WINDOW_MS = 5_000;

/**
 * The console: what the rig is, and the two controls that change it.
 *
 * These used to be a status strip across the top and a "3 · Flash" panel three columns away. That
 * put the sentence telling an operator what to do at one end of the screen and the button doing it
 * at the other — and worse, it made the status line look like decoration. It is not: `Ready` and
 * `No board` are the difference between a press that flashes and a press that does nothing, so the
 * button belongs inside the thing that says which it is.
 *
 * Everything that changes while a pass runs is here too — the progress bar, the confirm, the
 * arming note. An operator watching a production flash should not have to look anywhere else.
 */
function RigConsole({ rig }: { rig: RigState }) {
  const tile = tileFor(rig);
  const flashNow = useAction('/actions/flash_now');
  const flash = flashNowState(rig);
  const progress = stepSummary(rig);

  const last: LastPass = {
    outcome: useEnumName<Outcome>('/last/outcome', 'none'),
    detail: useText('/last/detail'),
    kind: useText('/last/kind'),
    step: useEnumName<Step>('/last/step', 'idle'),
    advice: useText('/last/advice'),
    mayHaveWritten: useFlag('/last/may_have_written'),
  };
  const result = lastPassSummary(last);

  // A flash pass rewrites the firmware pages while preserving durable board records. It is still
  // destructive to the installed firmware and sits next to harmless Read device, so it asks twice.
  //
  // Only here, in manual mode. Auto-flash is the rapid path and it is already gated by arming
  // (which additionally requires an empty fixture), so putting a confirm in front of every board
  // there would defeat the point of the mode without adding any safety it does not already have.
  const [armedPress, setArmedPress] = useState(false);
  useEffect(() => {
    if (!armedPress) return;
    const timer = window.setTimeout(() => setArmedPress(false), CONFIRM_WINDOW_MS);
    return () => window.clearTimeout(timer);
  }, [armedPress]);
  // Anything that changes what the press would *do* retracts it: a board lifted, an image
  // swapped, the probe dropped. A confirm is consent to one specific action.
  useEffect(() => {
    setArmedPress(false);
  }, [flash.enabled, rig.targetPresent, rig.hasImage]);

  return (
    <section className="rig-console" data-av-surface="device-state" data-tone={tile.tone}>
      <div className="rig-console-head">
        <div className="rig-status">
          <span className="rig-headline">{tile.headline}</span>
          <span className="rig-instruction">{tile.instruction}</span>
        </div>

        <div className="rig-controls" data-av-surface="operator-controls">
          <div className="rig-action">
            <Button
              onClick={() => {
                if (armedPress) {
                  setArmedPress(false);
                  flashNow();
                } else {
                  setArmedPress(true);
                }
              }}
              variant={armedPress ? 'danger' : 'primary'}
              disabled={!flash.enabled}
              title={flash.reason}
              busy={rig.busy}
              busyLabel={progress ? progress.label : 'Flashing…'}
            >
              {armedPress ? 'Confirm — erase and flash' : 'Flash now'}
            </Button>
            {!flash.enabled && flash.reason && <span className="fw-hint">{flash.reason}</span>}
          </div>

          {/* Separated by a rule rather than by space: this is a mode switch, not another
              button, and the two must not be reachable by the same careless click. */}
          <div className="rig-mode">
            <span className="rig-mode-label">Auto-flash</span>
            <Toggle path="/mode/desired" />
            <Badge tone={rig.armed ? 'active' : 'idle'}>{rig.armed ? 'armed' : 'off'}</Badge>
            <span className="rig-mode-hint">Hands-free: seat, tone, cycle, tone</span>
          </div>
        </div>
      </div>

      {/* A pass is seconds long and the middle of it is irreversible. The bar is full width and
          directly under the button that started it. */}
      {progress && (
        <div className="rig-progress" data-committed={progress.committed ? '' : undefined}>
          <div
            className="rig-progress-track"
            role="progressbar"
            aria-label={progress.label}
            aria-valuemin={progress.determinate ? 0 : undefined}
            aria-valuemax={progress.determinate ? 100 : undefined}
            aria-valuenow={
              progress.determinate ? Math.round(rig.stepFraction * 100) : undefined
            }
          >
            <div
              className="rig-progress-fill"
              data-indeterminate={progress.determinate ? undefined : ''}
              style={
                progress.determinate
                  ? { width: `${Math.max(1, Math.round(rig.stepFraction * 100))}%` }
                  : undefined
              }
            />
          </div>
          <span className="rig-progress-label">
            {progress.committed ? `${progress.label} — do not lift the board` : `${progress.label}…`}
          </span>
        </div>
      )}

      {armedPress && (
        <Banner tone="warn">
          This erases the whole chip before writing. Press again to go ahead.
        </Banner>
      )}
      {rig.mode === 'auto' && !rig.armed && !progress && (
        <Banner tone="info">Arming — the fixture must be empty first.</Banner>
      )}

      {/* The result of the last pass, which used to be nowhere. Hidden only until one has run —
          never replaced by the next pass's progress, because "why did that fail" is asked while
          the retry is already under way. */}
      {result && (
        <div className="rig-result" data-tone={result.tone}>
          <div className="rig-result-head">
            <span className="rig-result-heading">{result.heading}</span>
            {last.kind && <Badge tone={result.tone}>{last.kind}</Badge>}
          </div>
          {result.detail && <p className="rig-result-detail">{result.detail}</p>}
          {result.consequence && (
            <p className="rig-result-consequence" data-written={last.mayHaveWritten ? '' : undefined}>
              {result.consequence}
            </p>
          )}
          {result.advice && <p className="rig-result-advice">{result.advice}</p>}
        </div>
      )}
    </section>
  );
}

/**
 * Everything that is configuration or diagnostics rather than the operator's workflow.
 *
 * It used to be one collapsed `Parameters` section at the very bottom of the Session panel holding
 * a raw dump of every declared parameter, three levels deep. That read like a drawer of hidden
 * settings, and it was worth checking whether it was: it is not. Every writable parameter this
 * application declares already has a proper control somewhere on the Rig tab — the mode switch, the
 * probe list, the artefact rows, the three action buttons. What was genuinely hidden was the
 * opposite thing: facts about how the rig is configured that were on no parameter at all.
 *
 * So this tab is two halves. `/setup` is the answers, in words. Diagnostics is the tree, still
 * here because it is the only way to see a value nothing has got round to drawing — but grouped,
 * filterable, and no longer the first thing you meet.
 */
function SettingsTab({
  filter,
  setFilter,
  advanced,
  setAdvanced,
}: {
  filter: string;
  setFilter: (next: string) => void;
  advanced: boolean;
  setAdvanced: (next: boolean) => void;
}) {
  return (
    <div className="flasher-settings">
      <Panel title="How this rig is set up">
        <Banner tone="info">
          Read-only. Nothing here is a control — these are the values the rig is running with, which
          were previously true and invisible.
        </Banner>
        {/* Generated from the schema rather than hand-written rows. The labels and units are
            declared in Rust beside the code that publishes them, so a readout cannot drift from
            what it describes. */}
        <ParamTree prefix="/setup" />
      </Panel>

      <Panel
        title="All parameters"
        right={
          <>
            <ParamFilter value={filter} onChange={setFilter} />
            {/* The two belong together: the filter searches only what is *shown*, so a search for
                "serial" finds nothing until this is ticked. Separating them would make that read
                as a broken search. */}
            <label className="pref-toggle">
              <input
                type="checkbox"
                checked={advanced}
                onChange={(event) => setAdvanced(event.target.checked)}
              />
              Show list internals
            </label>
          </>
        }
      >
        <Banner tone="info">
          Every value on the bus, including ones no panel draws. Editing here is a diagnostic act,
          not a setting.
        </Banner>
        <ParamTree depth={2} advanced={advanced} filter={filter} />
      </Panel>
    </div>
  );
}

/**
 * What is on the board: the sector grid and everything read alongside it.
 *
 * The device detail used to be a separate panel from the map, which split one subject in two —
 * the grid showed *where* the flash was used and the panel next to it said *what* was there, and
 * neither made sense alone. Read device lives here too, because this panel is the only thing it
 * affects.
 */
function FlashContentsPanel({
  report,
  model,
  rig,
}: {
  report: DeviceJson | null;
  model: MapModel;
  rig: RigState;
}) {
  const layout = useEnumName<Layout>('/device/layout', 'unknown');
  const summary = layoutSummary(layout);
  const comparison = comparisonSummary(model);
  const readDevice = useAction('/actions/read_device');
  const read = readDeviceState(rig);

  return (
    <Panel
      title="Flash contents"
      right={
        report ? <Badge tone={summary.tone}>{summary.label}</Badge> : <Badge>not read</Badge>
      }
    >
      <div className="device-tools">
        <Button onClick={readDevice} disabled={!read.enabled} title={read.reason}>
          {report ? 'Re-read device' : 'Read device'}
        </Button>
        {!read.enabled && read.reason && <span className="fw-hint">{read.reason}</span>}
      </div>

      {!report ? (
        <EmptyState inline detail="Seat a board and press Read device." />
      ) : (
        <>
          <FirmwareMap model={model} />

          {/* Only when there is something to compare against. Saying "no image selected" here
              read as a complaint about the flash contents rather than about the picker. */}
          {comparison && (
            <Banner tone={comparison.tone === 'ok' ? 'info' : 'warn'}>{comparison.label}</Banner>
          )}

          <Row label="Layout" hint={summary.detail}>
            {summary.label}
          </Row>
          {report.flat_entry && <Row label="Entry">{report.flat_entry}</Row>}
          {report.regions.map((region) => (
            <Row key={region.name} label={region.name}>
              {region.used_bytes === 0
                ? 'erased'
                : `${formatBytes(region.used_bytes)}${region.banner ? ` · ${region.banner}` : ''}`}
            </Row>
          ))}
          <Row label="Programmed">
            {formatBytes(report.programmed_bytes)} of {formatBytes(report.total_bytes)}
          </Row>
          <Row label="UID">{report.uid}</Row>

          <Section title="Option bytes" defaultOpen={false}>
            <Row label="OPTR">{report.options.raw}</Row>
            <Row label="RDP">level {report.options.rdp_level}</Row>
            <Row label="nBOOT_SEL" hint="0 means BOOT0 comes from PA14, the probe's SWCLK">
              {report.options.nboot_sel ? '1' : '0'}
            </Row>
            <Row label="IWDG_SW">{report.options.iwdg_sw ? '1 (software)' : '0 (hardware)'}</Row>
            <Row label="NRST_MODE">{report.options.nrst_mode}</Row>
          </Section>
          {report.options.warnings.map((warning) => (
            <Banner key={warning} tone="warn">
              {warning}
            </Banner>
          ))}
        </>
      )}
    </Panel>
  );
}

// ---------------------------------------------------------------- the page

function App() {
  const schema = useSchema();
  const sounds = useMemo(() => new SystemSounds(), []);
  useHeartbeat();

  const mode = useEnumName<Mode>('/mode/desired', 'manual');
  const armed = useFlag('/autoflash/armed');
  const phase = useEnumName<Phase>('/rig/phase', 'disarmed');
  const expect = useEnumName<Expect>('/rig/expect', 'flash');
  const lastCue = useEnumName<Cue>('/rig/cue', 'none');
  const cueSeq = useNumber('/rig/cue_seq');
  const detail = useText('/rig/detail');
  const busy = useFlag('/rig/busy');
  const step = useEnumName<Step>('/rig/step', 'idle');
  const stepFraction = useNumber('/rig/step_fraction');

  const probeConnected = useFlag('/probe/connected');
  const imageName = useText('/image/name');
  const hasImage = imageName !== '' && imageName !== 'none';

  const passed = useNumber('/counts/passed');
  const failed = useNumber('/counts/failed');
  const faults = useNumber('/faults/active');

  const deviceRead = useFlag('/device/read');
  const report = useDeviceReport(deviceRead ? 1 : 0);

  useCueSounds(sounds, lastCue, cueSeq);

  // Straight from the poll, not inferred. Deriving it from the phase looked reasonable and was
  // a deadlock: in manual mode the phase never leaves `disarmed`, so Read device stayed disabled
  // until something had been read, which nothing could be.
  const targetPresent = useFlag('/probe/target_present');

  const rig: RigState = {
    mode,
    armed,
    phase,
    expect,
    lastCue,
    probeConnected,
    targetPresent,
    busy,
    hasImage,
    step,
    stepFraction,
  };

  const status = statusSummary(rig);
  const simulated = Boolean(schema?.params?.some?.((p) => p.path === '/sim/board_present'));

  // All three live here, above the tab boundary, and not inside the settings tree they belong to.
  // Anything owned below that boundary is destroyed on every switch: a filter you typed and a
  // "show internals" you ticked would both be gone after a glance at the rig and back, which reads
  // as a bug rather than as a design. (`Section`'s collapse state has the same problem and cannot
  // be lifted — it is uncontrolled with no id — which is why the sections here start closed.)
  const [tab, setTab] = useState<'rig' | 'settings'>('rig');
  const [paramFilter, setParamFilter] = useState('');
  const [paramAdvanced, setParamAdvanced] = useState(false);

  const model = useMemo(
    () =>
      buildMap(report?.occupancy ?? null, report?.selected_occupancy ?? null, {
        // The memory map comes from the Rust side rather than being restated here, so the
        // grid cannot drift from the part it is drawing.
        sectorBytes: report?.sector_bytes ?? 2048,
        bootloaderSectors: report?.bootloader_sectors ?? 12,
        flashBase: report ? Number.parseInt(report.flash_base, 16) : 0x0800_0000,
      }),
    [report],
  );

  return (
    <div className="app app--filled">
      <TitleBar
        title="Portal Flasher"
        sub={schema ? (simulated ? 'simulated target' : 'STM32G070RBT6 over SWD') : 'connecting…'}
      />
      <div className="app-body">
        {/*
          Every hook with a side effect stays in `App`, above this strip, and the tab bodies are
          presentation only. That is not tidiness: `useHeartbeat` is the dead man, and if it moved
          into the Rig tab then opening Settings would stop feeding it and the worker would disarm
          itself three seconds later, silently, while the operator was looking at a settings page.
          `useCueSounds` is the same shape — the fail tone would stop playing.
        */}
        <Tabs
          value={tab}
          onChange={setTab}
          label="Views"
          items={[
            // The fault count rides on the tab you are not looking at, so a failure while Settings
            // is open is visible without switching back.
            { id: 'rig', label: 'rig', count: faults > 0 ? faults : undefined },
            { id: 'settings', label: 'settings' },
          ]}
        />

        {tab === 'settings' ? (
          <SettingsTab
            filter={paramFilter}
            setFilter={setParamFilter}
            advanced={paramAdvanced}
            setAdvanced={setParamAdvanced}
          />
        ) : (
          <div className="flasher-layout">
            {/* Across the width: what the rig is, and the controls that change it. */}
            <RigConsole rig={rig} />

            {/* Setup, left to right. The action used to be a third panel here and is now above,
                where the sentence saying whether it will work already was. */}
            <div className="flasher-steps">
              <ProbePanel connected={probeConnected} />
              <section data-av-surface="image-source">
                <FirmwarePanel hasImage={hasImage} />
              </section>
            </div>

            {/* The evidence: what is on the board, and what has happened this session. */}
            <div className="flasher-detail">
              <FlashContentsPanel report={report} model={model} rig={rig} />
              <section data-av-surface="session-log">
                <Panel
                  title="Session"
                  right={faults > 0 ? <Badge tone="error">{faults} faults</Badge> : null}
                >
                  <Row label="Boards">
                    {passed} pass · {failed} fail
                  </Row>
                  {/* Faults were their own panel, which meant a page that was mostly "none". They
                      are a fact about the session, so they live with it. */}
                  <div data-av-surface="faults">
                    {faults > 0 ? (
                      <Banner tone="error">
                        {faults} fault{faults === 1 ? '' : 's'} this session
                        {detail ? `: ${detail}` : ''}
                      </Banner>
                    ) : detail ? (
                      <Row label="Detail">{detail}</Row>
                    ) : (
                      <Row label="Faults">none</Row>
                    )}
                  </div>
                  {/* The simulation switches stay here, on the Rig tab, and did not move to
                      Settings with the parameter tree. `Board in fixture` is flipped *while
                      watching the phase tile* — it is how the debounce and the removal gate are
                      exercised — and a fixture switch one tab away from the thing it drives would
                      be a worse tool than the one being replaced. They were never part of the
                      complaint: they are labelled toggles, not tree rows. */}
                  {simulated && (
                    <Section title="Simulation" defaultOpen>
                      <Row
                        label="Board in fixture"
                        hint="Stands in for seating and lifting a board"
                      >
                        <Toggle path="/sim/board_present" />
                      </Row>
                      <Row label="Fail the next pass">
                        <Toggle path="/sim/fail_next_pass" />
                      </Row>
                    </Section>
                  )}
                </Panel>
              </section>
            </div>
          </div>
        )}
      </div>
      <StatusBar stream={null}>
        <StatusItem
          label="rig"
          value={status.value}
          tone={status.tone === 'error' ? 'error' : 'ok'}
        />
        <StatusItem label="boards" value={`${passed} pass · ${failed} fail`} />
        {faults > 0 && <StatusItem label="faults" value={String(faults)} tone="error" />}
      </StatusBar>
    </div>
  );
}

mount(<App />);
