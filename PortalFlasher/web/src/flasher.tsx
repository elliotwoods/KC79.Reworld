/**
 * The rig's page.
 *
 * Ordered by what a reader needs first, the way the framework's own `router-status.tsx` is: the
 * verdict at the top, big enough to read across a bench, and the evidence underneath. An
 * operator using this properly is not looking at it — they are listening — so the screen's job
 * is to answer the question they have *after* a tone surprised them.
 *
 * Two things here are safety mechanisms rather than decoration:
 *
 * - **The heartbeat.** This page re-asserts `/arm/heartbeat` once a second and the worker
 *   disarms if it goes stale. Sound lives in the browser, so a rig nobody can hear must not
 *   stay armed; closing this tab disarms it within three seconds.
 * - **The arm cue.** Arming plays a tone deliberately. It satisfies the browser's autoplay
 *   gesture requirement on a real click, *and* it is the operator's only chance to notice that
 *   audio is muted before a board is flashed in silence — `SystemSounds` swallows play()
 *   rejections by design, so nothing else would tell them.
 */

import { useEffect, useMemo, useRef } from 'react';

import { SystemSounds } from '@auroravision/av-gui/calibration';
import {
  Badge,
  Banner,
  Panel,
  ParamTree,
  ReadOut,
  Row,
  Section,
  StatusBar,
  StatusItem,
  TitleBar,
  Toggle,
} from '@auroravision/av-gui/controls';
import { mount, useParam, useSchema } from '@auroravision/av-gui/runtime';
import '@auroravision/av-gui/styles.css';

import {
  type Cue,
  type Expect,
  type Phase,
  shortHash,
  soundFor,
  statusSummary,
  tileFor,
} from './flasher-model';

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

/**
 * Keep the rig's dead-man fed for as long as this page is running.
 *
 * A closed tab drops the session, which the worker also sees; this catches the other case, a
 * page whose script has wedged while its socket stays open.
 */
function useHeartbeat() {
  const beat = useParam<number>('/arm/heartbeat');
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

function App() {
  const schema = useSchema();
  const sounds = useMemo(() => new SystemSounds(), []);

  useHeartbeat();

  const armed = Boolean(useParam<boolean>('/arm/observed').value);
  const desired = Boolean(useParam<boolean>('/arm/desired').value);
  const phase = useEnumName<Phase>('/rig/phase', 'disarmed');
  const expect = useEnumName<Expect>('/rig/expect', 'flash');
  const cue = useEnumName<Cue>('/rig/cue', 'none');
  const cueSeq = useNumber('/rig/cue_seq');
  const detail = useText('/rig/detail');

  const probePresent = Boolean(useParam<boolean>('/probe/present').value);
  const probeName = useText('/probe/name');

  const passed = useNumber('/counts/passed');
  const failed = useNumber('/counts/failed');
  const faults = useNumber('/faults/active');

  const imageSource = useText('/image/source');
  const buildId = useText('/image/build_id');
  const bootSha = useText('/image/boot_sha');
  const appSha = useText('/image/app_sha');

  const simulated = Boolean(schema?.params?.some?.((p) => p.path === '/sim/board_present'));

  useCueSounds(sounds, cue, cueSeq);

  const tile = tileFor(phase, expect, cue);
  const status = statusSummary(armed, phase, probePresent);

  return (
    <div className="app app--filled">
      <TitleBar
        title="Portal Flasher"
        sub={schema ? (simulated ? 'simulated target' : 'STM32G070RBT6 over SWD') : 'connecting…'}
      />
      <main className="app-body">
        {/* The verdict. Everything else on this page is evidence for it. */}
        <section data-av-surface="device-state">
          <Panel
            title="Rig"
            right={<Badge tone={probePresent ? 'ok' : 'error'}>{probeName || 'no probe'}</Badge>}
          >
            <div className="rig-tile" data-tone={tile.tone}>
              <div className="rig-headline">{tile.headline}</div>
              <div className="rig-instruction">{tile.instruction}</div>
            </div>
            <Row label="Passed">
              <ReadOut path="/counts/passed" />
            </Row>
            <Row label="Failed">
              <ReadOut path="/counts/failed" />
            </Row>
          </Panel>
        </section>

        <section data-av-surface="operator-controls">
          <Panel title="Control">
            {/* Desired and observed are drawn separately and deliberately: they disagree for a
                whole pass every time a disarm arrives mid-write, because aborting a write to
                honour a button is worse than finishing it. */}
            <Row label="Arm" hint="Requires an empty fixture before the first pass">
              <Toggle path="/arm/desired" />
            </Row>
            <Row label="Actually armed">
              <Badge tone={armed ? 'ok' : 'idle'}>{armed ? 'armed' : 'disarmed'}</Badge>
            </Row>
            {desired !== armed && (
              <Banner tone="warn">
                {desired
                  ? 'Arming — the fixture must be empty first.'
                  : 'Disarming when the pass in progress finishes. A write is never interrupted.'}
              </Banner>
            )}
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
          </Panel>
        </section>

        <section data-av-surface="image-source">
          <Panel
            title="Image"
            right={imageSource ? <Badge tone={imageSource === 'built' ? 'ok' : 'idle'}>{imageSource}</Badge> : null}
          >
            {imageSource === 'synthetic' && (
              <Banner tone="warn">
                A synthetic image. Nothing here corresponds to real firmware.
              </Banner>
            )}
            <Row label="Build">{buildId || '—'}</Row>
            <Row label="Bootloader">{shortHash(bootSha)}</Row>
            <Row label="Application">{shortHash(appSha)}</Row>
          </Panel>
        </section>

        <section data-av-surface="faults">
          <Panel title="Faults">
            {faults > 0 ? (
              <Banner tone="error">
                {faults} fault{faults === 1 ? '' : 's'} this session
                {detail ? `: ${detail}` : ''}
              </Banner>
            ) : (
              <Row label="Faults">none</Row>
            )}
            {detail && faults === 0 && <Row label="Detail">{detail}</Row>}
          </Panel>
        </section>

        {/* Evidence, one disclosure down: available without being in the way. */}
        <section data-av-surface="session-log">
          <Panel title="Session">
            <Section title="Parameters" defaultOpen={false}>
              <ParamTree />
            </Section>
          </Panel>
        </section>
      </main>
      <StatusBar stream={null}>
        <StatusItem label="rig" value={status.value} tone={status.tone === 'error' ? 'error' : 'ok'} />
        <StatusItem label="boards" value={`${passed} pass · ${failed} fail`} />
        {faults > 0 && <StatusItem label="faults" value={String(faults)} tone="error" />}
      </StatusBar>
    </div>
  );
}

mount(<App />);
