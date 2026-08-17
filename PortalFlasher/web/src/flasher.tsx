/**
 * The rig's page.
 *
 * Laid out as the order of operations, left to right: **probe → firmware → flash** down the left
 * column, and what is actually on the board down the right. An operator working properly is not
 * looking at it — they are listening — so the screen's job is to answer the question they have
 * *after* a tone surprised them, and to make the setup obvious before they start.
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
  ParamTree,
  Row,
  Section,
  StatusBar,
  StatusItem,
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
  type Layout,
  type Mode,
  type Phase,
  type RigState,
  type Step,
  flashNowState,
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

function FlashPanel({ rig }: { rig: RigState }) {
  const flashNow = useAction('/actions/flash_now');
  const flash = flashNowState(rig);
  const progress = stepSummary(rig);

  // A flash pass is a chip erase. The button is the only thing standing between a misclick and an
  // unrecoverable board, and it sits next to Read device, which is harmless -- so it asks twice.
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
    <Panel title="3 · Flash">
      <div className="device-tools">
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
      {armedPress && (
        <Banner tone="warn">
          This erases the whole chip before writing. Press again to go ahead.
        </Banner>
      )}
      {progress && (
        <Banner tone={progress.committed ? 'warn' : 'info'}>
          {progress.committed
            ? `${progress.label} — do not lift the board`
            : `${progress.label}…`}
        </Banner>
      )}
      <Row label="Auto-flash" hint="Hands-free: seat, tone, cycle, tone">
        <Toggle path="/mode/desired" />
      </Row>
      <Row label="Armed">
        <Badge tone={rig.armed ? 'active' : 'idle'}>{rig.armed ? 'armed' : 'off'}</Badge>
      </Row>
      {rig.mode === 'auto' && !rig.armed && (
        <Banner tone="info">Arming — the fixture must be empty first.</Banner>
      )}
    </Panel>
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

  const tile = tileFor(rig);
  const status = statusSummary(rig);
  const simulated = Boolean(schema?.params?.some?.((p) => p.path === '/sim/board_present'));

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
      <div className="app-body flasher-layout">
        {/* One line, across the width: what the rig is, and what to do about it. */}
        <section data-av-surface="device-state">
          <div className="rig-strip" data-tone={tile.tone}>
            <span className="rig-headline">{tile.headline}</span>
            <span className="rig-instruction">{tile.instruction}</span>
          </div>
        </section>

        {/* The order of operations, left to right. */}
        <div className="flasher-steps">
          <ProbePanel connected={probeConnected} />
          <section data-av-surface="image-source">
            <FirmwarePanel hasImage={hasImage} />
          </section>
          <section data-av-surface="operator-controls">
            <FlashPanel rig={rig} />
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
              {/* Faults were their own panel, which meant a page that was mostly "none". They are
                  a fact about the session, so they live with it. */}
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
              {simulated && (
                <Section title="Simulation" defaultOpen>
                  <Row label="Board in fixture" hint="Stands in for seating and lifting a board">
                    <Toggle path="/sim/board_present" />
                  </Row>
                  <Row label="Fail the next pass">
                    <Toggle path="/sim/fail_next_pass" />
                  </Row>
                </Section>
              )}
              <Section title="Parameters" defaultOpen={false}>
                <ParamTree />
              </Section>
            </Panel>
          </section>
        </div>
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
