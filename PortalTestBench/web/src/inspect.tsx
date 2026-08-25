import { Badge, Banner, Button, Panel } from '@auroravision/av-gui/controls';
import { useCallback, useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from 'react';

type Axis = 'a' | 'b';
type Mode = 'fast' | 'settled';
type Sample = { index: number; position: number; offset: number; crossing: number | null; class: string };
type Survey = { running: boolean; expected: number; samples: Sample[]; aborted: boolean; detail: string };
type InspectState = {
  direct: { mode: string; detail: string };
  survey: Survey;
  dut: {
    a: { position: number | null; health: { home_ok: boolean } | null };
    b: { position: number | null; health: { home_ok: boolean } | null };
  };
};

async function post(body: object) {
  const response = await fetch('/api/bench/command', {
    method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body),
  });
  if (!response.ok) {
    const result = await response.json().catch(() => ({})) as { error?: string };
    throw new Error(result.error ?? `request failed (${response.status})`);
  }
}

function Chart({ survey }: { survey: Survey }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const el = canvas.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    const ratio = window.devicePixelRatio || 1;
    el.width = Math.round(rect.width * ratio);
    el.height = Math.round(rect.height * ratio);
    const ctx = el.getContext('2d');
    if (!ctx) return;
    ctx.scale(ratio, ratio);
    const w = rect.width, h = rect.height, l = 52, r = 18, t = 18, b = 34;
    const offsets = survey.samples.map((s) => s.offset);
    const lo = offsets.length ? Math.min(...offsets) : -500;
    const hi = offsets.length ? Math.max(...offsets) : 500;
    const x = (n: number) => l + (n - lo) / Math.max(1, hi - lo) * (w - l - r);
    const y = (n: number) => t + (255 - n) / 55 * (h - t - b);
    ctx.clearRect(0, 0, w, h);
    ctx.fillStyle = '#0c1118'; ctx.fillRect(l, t, w - l - r, h - t - b);
    ctx.strokeStyle = 'rgba(170,190,210,.16)';
    ctx.fillStyle = '#8d9bab'; ctx.font = '11px ui-monospace, monospace';
    for (const duty of [200, 220, 240, 255]) {
      const py = y(duty); ctx.beginPath(); ctx.moveTo(l, py); ctx.lineTo(w - r, py); ctx.stroke(); ctx.fillText(String(duty), 16, py + 4);
    }
    for (const fraction of [0, .25, .5, .75, 1]) {
      const value = Math.round(lo + (hi - lo) * fraction), px = x(value);
      ctx.beginPath(); ctx.moveTo(px, t); ctx.lineTo(px, h - b); ctx.stroke(); ctx.fillText(String(value), px - 14, h - 12);
    }
    ctx.fillText('duty', 8, 12); ctx.fillText('offset from center (usteps)', Math.max(l, w / 2 - 75), h - 2);
    ctx.strokeStyle = '#55b7ff'; ctx.lineWidth = 2; ctx.beginPath();
    let open = false;
    for (const s of survey.samples) {
      if (s.crossing == null) { open = false; continue; }
      if (open) ctx.lineTo(x(s.offset), y(s.crossing)); else ctx.moveTo(x(s.offset), y(s.crossing));
      open = true;
    }
    ctx.stroke();
    for (const s of survey.samples) {
      const py = y(s.crossing ?? (s.class === 'censored_bright' ? 200 : 255));
      ctx.fillStyle = s.class === 'measured' ? '#7fd0ff' : s.class === 'failed' ? '#ff6b78' : '#f4bd58';
      ctx.beginPath();
      if (s.crossing == null) { ctx.moveTo(x(s.offset), py - 5); ctx.lineTo(x(s.offset) - 4, py + 3); ctx.lineTo(x(s.offset) + 4, py + 3); }
      else ctx.arc(x(s.offset), py, 3, 0, Math.PI * 2);
      ctx.fill();
    }
  }, [survey]);
  return <canvas ref={canvas} className="survey-chart" aria-label="Optical threshold crossing by position" />;
}

function Jog({ axis, disabled }: { axis: Axis; disabled: boolean }) {
  const [speed, setSpeed] = useState(0);
  const last = useRef(0);
  const send = useCallback((next: number) => {
    setSpeed(next);
    const now = performance.now();
    if (next !== 0 && now - last.current < 40) return;
    last.current = now;
    void post({ op: 'jog', axis, speed: next });
  }, [axis]);
  const update = (event: ReactPointerEvent<HTMLDivElement>) => {
    const rect = event.currentTarget.getBoundingClientRect();
    const n = Math.max(-1, Math.min(1, ((event.clientX - rect.left) / rect.width) * 2 - 1));
    send(Math.round(Math.sign(n) * n * n * 14_080));
  };
  const down = (event: ReactPointerEvent<HTMLDivElement>) => { event.currentTarget.setPointerCapture(event.pointerId); update(event); };
  const up = (event: ReactPointerEvent<HTMLDivElement>) => { if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId); send(0); };
  useEffect(() => () => { void post({ op: 'jog', axis, speed: 0 }); }, [axis]);
  const marker = 50 + 50 * Math.sign(speed) * Math.sqrt(Math.abs(speed) / 14_080);
  return <div className="jog-control">
    <div className="jog-copy"><strong>Velocity jog</strong><span>{speed.toLocaleString()} usteps/s</span></div>
    <div className="jog-strip" data-disabled={disabled || undefined}
      onPointerDown={disabled ? undefined : down}
      onPointerMove={(event) => { if (event.currentTarget.hasPointerCapture(event.pointerId)) update(event); }}
      onPointerUp={up} onPointerCancel={up} role="slider" aria-label="Velocity jog"
      aria-valuemin={-14080} aria-valuemax={14080} aria-valuenow={speed}>
      <span>reverse</span><i /><span>forward</span><b style={{ left: `${marker}%` }} />
    </div>
    <small>Press anywhere for speed and drag while held. Quadratic response gives fine control near zero.</small>
  </div>;
}

export function InspectTab() {
  const [state, setState] = useState<InspectState | null>(null);
  const [axis, setAxis] = useState<Axis>('a'), [mode, setMode] = useState<Mode>('settled');
  const [center, setCenter] = useState(0), [halfRange, setHalfRange] = useState(500), [step, setStep] = useState(10);
  const [dutyMin, setDutyMin] = useState(200), [dutyMax, setDutyMax] = useState(255), [error, setError] = useState('');
  const refresh = useCallback(async () => {
    const response = await fetch('/api/bench/state', { cache: 'no-store' });
    if (response.ok) setState(await response.json() as InspectState);
  }, []);
  useEffect(() => { void refresh(); const id = window.setInterval(() => void refresh(), 200); return () => window.clearInterval(id); }, [refresh]);
  const run = async (body: object) => { setError(''); try { await post(body); await refresh(); } catch (reason) { setError(reason instanceof Error ? reason.message : String(reason)); } };
  const direct = state?.direct.mode === 'direct';
  const survey = state?.survey ?? { running: false, expected: 0, samples: [], aborted: false, detail: '' };
  const selected = state?.dut[axis], position = selected?.position, homed = !!selected?.health?.home_ok;
  const expected = halfRange > 0 && step > 0 ? Math.floor(halfRange * 2 / step) + 1 : 0;
  const start = () => void run({ op: 'start_survey', config: {
    axis, mode, center, center_is_home: homed && center === 0, half_range: halfRange, step, duty_min: dutyMin, duty_max: dutyMax,
  } });
  return <div className="inspect-workspace">
    {error && <Banner tone="error">{error}</Banner>}
    <section className="direct-bar">
      <div><span className="label-caps">Serial session</span><strong>{direct ? 'Direct Mode' : 'Human menu'}</strong><small>{state?.direct.detail ?? 'Connect production VCOM first'}</small></div>
      <Badge tone={direct ? 'ok' : 'idle'}>{direct ? 'binary heartbeat active' : 'text console'}</Badge>
      {direct ? <Button disabled={survey.running} onClick={() => void run({ op: 'exit_direct' })}>Exit Direct Mode</Button>
        : <Button variant="primary" onClick={() => void run({ op: 'enter_direct' })}>Enter Direct Mode</Button>}
    </section>
    <div className="inspect-grid">
      <Panel title="Survey setup" right={<Badge tone={survey.running ? 'warn' : 'idle'}>{survey.running ? 'running' : `${expected} samples`}</Badge>}>
        <div className="inspect-fields">
          <label><span>Axis</span><select value={axis} disabled={survey.running} onChange={(e) => setAxis(e.target.value as Axis)}><option value="a">Axis A</option><option value="b">Axis B</option></select></label>
          <label><span>Measurement</span><select value={mode} disabled={survey.running} onChange={(e) => setMode(e.target.value as Mode)}><option value="settled">Settled / accurate</option><option value="fast">Fast / lagged profile</option></select></label>
          <label><span>Center (usteps)</span><input type="number" value={center} disabled={survey.running} onChange={(e) => setCenter(Number(e.target.value))} /></label>
          <div className="center-tools"><Button disabled={survey.running || position == null} onClick={() => setCenter(position ?? 0)}>Capture current ({position ?? '-'})</Button><Button disabled={survey.running || !homed} onClick={() => setCenter(0)}>Use home (0)</Button></div>
          <label><span>Range +/- usteps</span><input type="number" min="1" max="20000" value={halfRange} disabled={survey.running} onChange={(e) => setHalfRange(Number(e.target.value))} /></label>
          <label><span>Step (usteps)</span><input type="number" min="1" value={step} disabled={survey.running} onChange={(e) => setStep(Number(e.target.value))} /></label>
          <label><span>Duty from</span><input type="number" min="0" max="255" value={dutyMin} disabled={survey.running} onChange={(e) => setDutyMin(Number(e.target.value))} /></label>
          <label><span>Duty to</span><input type="number" min="0" max="255" value={dutyMax} disabled={survey.running} onChange={(e) => setDutyMax(Number(e.target.value))} /></label>
        </div>
        {mode === 'fast' && <Banner tone="warn">Fast mode shows the RC-lagged profile. Use Settled for threshold decisions.</Banner>}
        {!homed && <Banner tone="info">This axis is not homed. Jog into the flag region, then capture the current center.</Banner>}
        <Jog axis={axis} disabled={!direct || survey.running} />
        <div className="survey-actions"><Button variant="primary" disabled={!direct || survey.running || expected > 4096 || dutyMin > dutyMax} onClick={start}>Start optical survey</Button><Button variant="danger" disabled={!survey.running} onClick={() => void run({ op: 'escape', channel: 'serial' })}>Abort</Button></div>
      </Panel>
      <Panel title="Home flag profile" right={<span className="survey-export"><a href="/api/bench/survey/export.csv" download>CSV</a><a href="/api/bench/survey/export.json" download>JSON</a></span>}>
        <div className="survey-progress"><span>{survey.detail || 'Ready'}</span><strong>{survey.samples.length} / {survey.expected || expected}</strong></div>
        <progress max={Math.max(1, survey.expected || expected)} value={survey.samples.length} />
        <Chart survey={survey} />
        <div className="survey-legend"><span><i className="measured" /> measured</span><span><i className="censored" /> censored</span><span><i className="failed" /> failed</span></div>
      </Panel>
    </div>
  </div>;
}
