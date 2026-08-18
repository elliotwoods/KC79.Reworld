import { Button, NumberField, Row } from '@auroravision/av-gui/controls';
import { useParam } from '@auroravision/av-gui/runtime';
import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from 'react';

interface Sample {
  seq: number;
  at_ms: number;
  channel: 'serial' | 'rs485';
  target_id?: number;
  axis: 'a' | 'b';
  position: number;
  target?: number;
}

function css(name: string): string {
  const style = getComputedStyle(document.documentElement);
  return style.getPropertyValue(name).trim() || style.getPropertyValue('--text').trim();
}

function fit(canvas: HTMLCanvasElement): CanvasRenderingContext2D | null {
  const dpr = window.devicePixelRatio || 1;
  const rect = canvas.getBoundingClientRect();
  const w = Math.max(1, Math.round(rect.width * dpr));
  const h = Math.max(1, Math.round(rect.height * dpr));
  if (canvas.width !== w || canvas.height !== h) {
    canvas.width = w;
    canvas.height = h;
  }
  const ctx = canvas.getContext('2d');
  ctx?.setTransform(dpr, 0, 0, dpr, 0, 0);
  return ctx;
}

/** A local-authority XY pilot: pointer motion stays in the canvas; one command is committed on release. */
export function MotionPilot() {
  const canvas = useRef<HTMLCanvasElement>(null);
  const a = useParam<number>('/motion/a/rotations');
  const b = useParam<number>('/motion/b/rotations');
  const draft = useRef({ a: a.value ?? 0, b: b.value ?? 0, dragging: false });

  useEffect(() => {
    if (!draft.current.dragging) draft.current = { a: a.value ?? 0, b: b.value ?? 0, dragging: false };
  }, [a.value, b.value]);

  useEffect(() => {
    let raf = 0;
    const draw = () => {
      const el = canvas.current;
      if (el) {
        const ctx = fit(el);
        if (ctx) {
          const { width: w, height: h } = el.getBoundingClientRect();
          ctx.clearRect(0, 0, w, h);
          ctx.strokeStyle = css('--border');
          ctx.lineWidth = 1;
          ctx.beginPath(); ctx.moveTo(w / 2, 10); ctx.lineTo(w / 2, h - 10); ctx.stroke();
          ctx.beginPath(); ctx.moveTo(10, h / 2); ctx.lineTo(w - 10, h / 2); ctx.stroke();
          ctx.fillStyle = css('--text-faint');
          ctx.font = '11px sans-serif';
          ctx.fillText('−A', 10, h / 2 - 7); ctx.fillText('+A', w - 27, h / 2 - 7);
          ctx.fillText('+B', w / 2 + 7, 17); ctx.fillText('−B', w / 2 + 7, h - 10);
          const x = w / 2 + Math.max(-2, Math.min(2, draft.current.a)) * (w - 32) / 4;
          const y = h / 2 - Math.max(-2, Math.min(2, draft.current.b)) * (h - 32) / 4;
          ctx.fillStyle = css('--accent');
          ctx.beginPath(); ctx.arc(x, y, 9, 0, Math.PI * 2); ctx.fill();
        }
      }
      raf = requestAnimationFrame(draw);
    };
    raf = requestAnimationFrame(draw);
    return () => cancelAnimationFrame(raf);
  }, []);

  const update = (event: ReactPointerEvent<HTMLCanvasElement>) => {
    const rect = event.currentTarget.getBoundingClientRect();
    draft.current.a = Math.max(-2, Math.min(2, ((event.clientX - rect.left) / rect.width - 0.5) * 4));
    draft.current.b = Math.max(-2, Math.min(2, (0.5 - (event.clientY - rect.top) / rect.height) * 4));
  };
  const down = (event: ReactPointerEvent<HTMLCanvasElement>) => {
    event.currentTarget.setPointerCapture(event.pointerId);
    draft.current.dragging = true;
    update(event);
  };
  const up = (event: ReactPointerEvent<HTMLCanvasElement>) => {
    update(event);
    draft.current.dragging = false;
    a.set(Number(draft.current.a.toFixed(4)));
    b.set(Number(draft.current.b.toFixed(4)));
  };

  return (
    <div className="pilot-grid">
      <canvas ref={canvas} className="pilot" aria-label="Axis A and B rotation pilot" onPointerDown={down} onPointerMove={(e) => draft.current.dragging && update(e)} onPointerUp={up} />
      <div className="pilot-fields">
        <Row label="Axis A [rev]"><NumberField path="/motion/a/rotations" /></Row>
        <Row label="Axis B [rev]"><NumberField path="/motion/b/rotations" /></Row>
        <Row label="Max velocity"><NumberField path="/motion/profile/max_velocity" /></Row>
        <Row label="Acceleration"><NumberField path="/motion/profile/acceleration" /></Row>
        <Row label="Min velocity"><NumberField path="/motion/profile/min_velocity" /></Row>
      </div>
    </div>
  );
}

type Derived = { at: number; a?: number; b?: number };

function derive(samples: Sample[], kind: 'position' | 'velocity' | 'acceleration'): Derived[] {
  const prior = new Map<string, { at: number; p: number; v?: number }>();
  return samples.map((s) => {
    const key = `${s.channel}:${s.target_id ?? ''}:${s.axis}`;
    const old = prior.get(key);
    const dt = old ? (s.at_ms - old.at) / 1000 : 0;
    const velocity = old && dt > 0 ? (s.position - old.p) / dt : undefined;
    const acceleration = old?.v !== undefined && velocity !== undefined && dt > 0 ? (velocity - old.v) / dt : undefined;
    prior.set(key, { at: s.at_ms, p: s.position, v: velocity });
    const value = kind === 'position' ? s.position : kind === 'velocity' ? velocity : acceleration;
    return { at: s.at_ms, [s.axis]: value };
  });
}

function drawSeries(ctx: CanvasRenderingContext2D, data: Derived[], field: 'a' | 'b', color: string, x0: number, span: number, w: number, top: number, h: number, maxAbs: number) {
  ctx.strokeStyle = color;
  ctx.lineWidth = 1.5;
  ctx.beginPath();
  let started = false;
  for (const point of data) {
    const value = point[field];
    if (value === undefined || !Number.isFinite(value)) continue;
    const x = ((point.at - x0) / span) * w;
    const y = top + h / 2 - (value / maxAbs) * (h * 0.42);
    if (started) ctx.lineTo(x, y); else { ctx.moveTo(x, y); started = true; }
  }
  ctx.stroke();
}

/** 10 Hz fetch into a closure-owned ring; samples never enter React state. */
export function MotionGraphs() {
  const canvas = useRef<HTMLCanvasElement>(null);
  const samples = useRef<Sample[]>([]);
  const cursor = useRef(0);
  const pausedRef = useRef(false);
  const [paused, setPaused] = useState(false);
  pausedRef.current = paused;

  useEffect(() => {
    let stopped = false;
    const poll = async () => {
      if (stopped || pausedRef.current) return;
      try {
        const response = await fetch(`/api/bench/telemetry?from=${cursor.current}`, { cache: 'no-store' });
        if (!response.ok) return;
        const body = await response.json() as { next: number; samples: Sample[] };
        cursor.current = body.next;
        samples.current.push(...body.samples);
        const newest = samples.current.at(-1)?.at_ms ?? 0;
        samples.current = samples.current.filter((s) => s.at_ms >= newest - 60_000).slice(-8192);
      } catch { /* the next tick retries */ }
    };
    const timer = setInterval(poll, 100);
    void poll();
    return () => { stopped = true; clearInterval(timer); };
  }, []);

  useEffect(() => {
    let raf = 0;
    const draw = () => {
      const el = canvas.current;
      if (el) {
        const ctx = fit(el);
        if (ctx) {
          const { width: w, height: h } = el.getBoundingClientRect();
          ctx.clearRect(0, 0, w, h);
          const rows = ['position', 'velocity', 'acceleration'] as const;
          const rowH = h / 3;
          const newest = samples.current.at(-1)?.at_ms ?? 60_000;
          const x0 = newest - 60_000;
          for (let i = 0; i < rows.length; i++) {
            const data = derive(samples.current, rows[i]);
            const values = data.flatMap((p) => [p.a, p.b]).filter((v): v is number => v !== undefined && Number.isFinite(v));
            const maxAbs = Math.max(1, ...values.map(Math.abs));
            const top = i * rowH;
            ctx.strokeStyle = css('--border');
            ctx.beginPath(); ctx.moveTo(0, top + rowH / 2); ctx.lineTo(w, top + rowH / 2); ctx.stroke();
            ctx.fillStyle = css('--text-secondary');
            ctx.font = '11px sans-serif';
            ctx.fillText(`${rows[i]}  ±${Math.round(maxAbs).toLocaleString()} µsteps${i === 0 ? '' : i === 1 ? '/s' : '/s²'}`, 7, top + 14);
            drawSeries(ctx, data, 'a', css('--accent'), x0, 60_000, w, top, rowH, maxAbs);
            drawSeries(ctx, data, 'b', css('--ok'), x0, 60_000, w, top, rowH, maxAbs);
          }
        }
      }
      raf = requestAnimationFrame(draw);
    };
    raf = requestAnimationFrame(draw);
    return () => cancelAnimationFrame(raf);
  }, []);

  return (
    <div className="motion-graphs">
      <div className="graph-tools"><span><i className="legend-a" /> A <i className="legend-b" /> B · measured, 60 s</span><Button variant="quiet" onClick={() => setPaused((v) => !v)}>{paused ? 'Resume' : 'Pause'}</Button><Button variant="quiet" onClick={() => { samples.current = []; cursor.current = 0; }}>Clear</Button></div>
      <canvas ref={canvas} className="motion-chart" aria-label="Measured position velocity and acceleration for axes A and B" />
    </div>
  );
}
