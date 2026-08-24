// Interaction canvases: the pilot disk, the axis dials, the pilot-all pads and the image
// preview. All follow the framework's fast-path rules: 2D canvas drawn in a rAF loop from
// telemetry rings and refs, drag-authority (the widget owns its value while under the
// pointer and streams writes), and no React state on the drawing path.

import { useParam } from '@auroravision/av-gui/runtime';
import {
  useEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from 'react';
import {
  DISK_WARP,
  clampToUnitCircle,
  dialAngle,
  dialValue,
  diskPosition,
} from './math';
import { latestRow, useRing, useVec2 } from './model';

// ------------------------------------------------------------------ shared utils

export function css(name: string, fallback = ''): string {
  const style = getComputedStyle(document.documentElement);
  return style.getPropertyValue(name).trim() || fallback || style.getPropertyValue('--text').trim();
}

export function fit(canvas: HTMLCanvasElement): CanvasRenderingContext2D | null {
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

/** Canvas palette, resolved once per mount (framework tokens + router extras). */
export interface Palette {
  gridLine: string;
  liveBlue: string;
  targetWhite: string;
  accent: string;
  ok: string;
  warn: string;
  error: string;
  textMuted: string;
  surfaceTint: string;
}

export function palette(): Palette {
  return {
    gridLine: css('--router-grid-line', 'rgba(255,255,255,0.13)'),
    liveBlue: css('--router-live', '#4c8cff'),
    targetWhite: css('--router-target', '#f2f2f2'),
    accent: css('--accent', '#4f8ef7'),
    ok: css('--router-ok', '#3fcf6e'),
    warn: css('--warn', '#f2b52e'),
    error: css('--danger', '#e5484d'),
    textMuted: css('--text-muted', 'rgba(232,234,240,0.55)'),
    surfaceTint: css('--router-surface-tint', 'rgba(255,255,255,0.03)'),
  };
}

export function useRafDraw(draw: () => void) {
  const drawRef = useRef(draw);
  drawRef.current = draw;
  useEffect(() => {
    let raf = 0;
    const loop = () => {
      drawRef.current();
      raf = requestAnimationFrame(loop);
    };
    raf = requestAnimationFrame(loop);
    return () => cancelAnimationFrame(raf);
  }, []);
}

/** Lanes of `/tel/portal/selected` (see schema.rs). */
export const SEL = {
  posX: 0,
  posY: 1,
  polarR: 2,
  polarTheta: 3,
  axisA: 4,
  axisB: 5,
  liveA: 6,
  liveB: 7,
  liveX: 8,
  liveY: 9,
  liveTgtX: 10,
  liveTgtY: 11,
  mcPosA: 12,
  mcTgtA: 13,
  mcPosB: 14,
  mcTgtB: 15,
  inPos: 16,
  rxAge: 17,
} as const;

// ------------------------------------------------------------------ pilot disk

/**
 * The polar disk: pow(r, 0.4) warp, rings at .25/.5/.75/1, crosshair, 30° ticks, +x/+y
 * labels, live dot (blue) / live-target ring (blue) / local target ring+dot (white), dashed
 * leader while live and target disagree. Drag to move (streamed); double-click to jump.
 */
export function PilotDisk({ size = 300 }: { size?: number }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const position = useParam<number[]>('/portal/pilot/position');
  const ring = useRing('/tel/portal/selected');
  const draft = useRef<{ dragging: boolean; x: number; y: number }>({
    dragging: false,
    x: 0,
    y: 0,
  });

  const value = position.value;
  useEffect(() => {
    if (!draft.current.dragging && Array.isArray(value)) {
      draft.current.x = value[0] ?? 0;
      draft.current.y = value[1] ?? 0;
    }
  }, [value]);

  useRafDraw(() => {
    const el = canvas.current;
    if (!el) return;
    const ctx = fit(el);
    if (!ctx) return;
    const pal = palette();
    const { width: w, height: h } = el.getBoundingClientRect();
    const cx = w / 2;
    const cy = h / 2;
    const radius = Math.min(w, h) / 2 - 14;
    ctx.clearRect(0, 0, w, h);

    // background disc + warped rings + crosshair + ticks
    ctx.fillStyle = pal.surfaceTint;
    ctx.beginPath();
    ctx.arc(cx, cy, radius, 0, Math.PI * 2);
    ctx.fill();
    ctx.strokeStyle = pal.gridLine;
    ctx.lineWidth = 1;
    for (const r of [0.25, 0.5, 0.75, 1]) {
      ctx.beginPath();
      ctx.arc(cx, cy, r ** DISK_WARP * radius, 0, Math.PI * 2);
      ctx.stroke();
    }
    ctx.beginPath();
    ctx.moveTo(cx - radius, cy);
    ctx.lineTo(cx + radius, cy);
    ctx.moveTo(cx, cy - radius);
    ctx.lineTo(cx, cy + radius);
    ctx.stroke();
    for (let i = 0; i < 12; i++) {
      const angle = (i * Math.PI * 2) / 12;
      ctx.beginPath();
      ctx.moveTo(cx + Math.cos(angle) * (radius - 4), cy + Math.sin(angle) * (radius - 4));
      ctx.lineTo(cx + Math.cos(angle) * radius, cy + Math.sin(angle) * radius);
      ctx.stroke();
    }
    ctx.fillStyle = pal.textMuted;
    ctx.font = '10px var(--font-sans, sans-serif)';
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    ctx.fillText('+y', cx, cy - radius - 8);
    ctx.fillText('+x', cx + radius + 9, cy);

    const toScreen = (x: number, y: number) => {
      const r = Math.hypot(x, y);
      const scale = r > 0 ? r ** DISK_WARP / r : 0;
      return { x: cx + x * scale * radius, y: cy - y * scale * radius };
    };

    const target = { x: draft.current.x, y: draft.current.y };
    const tp = toScreen(target.x, target.y);

    // overflow line: |target| > 1 marks the polar cycle boundary (C++ behaviour)
    if (Math.hypot(target.x, target.y) > 1.0001) {
      ctx.strokeStyle = pal.warn;
      ctx.beginPath();
      ctx.moveTo(cx - radius, cy);
      ctx.lineTo(cx, cy);
      ctx.stroke();
    }

    const row = latestRow(ring);
    const live =
      row && Number.isFinite(row[SEL.liveX])
        ? toScreen(row[SEL.liveX], row[SEL.liveY])
        : null;
    const liveTgt =
      row && Number.isFinite(row[SEL.liveTgtX])
        ? toScreen(row[SEL.liveTgtX], row[SEL.liveTgtY])
        : null;

    if (live && Math.hypot(live.x - tp.x, live.y - tp.y) > 4) {
      ctx.save();
      ctx.setLineDash([3, 4]);
      ctx.strokeStyle = 'rgba(255,255,255,0.35)';
      ctx.beginPath();
      ctx.moveTo(live.x, live.y);
      ctx.lineTo(tp.x, tp.y);
      ctx.stroke();
      ctx.restore();
    }
    if (live) {
      ctx.fillStyle = pal.liveBlue;
      ctx.beginPath();
      ctx.arc(live.x, live.y, 7, 0, Math.PI * 2);
      ctx.fill();
    }
    if (liveTgt) {
      ctx.strokeStyle = pal.liveBlue;
      ctx.lineWidth = 2;
      ctx.beginPath();
      ctx.arc(liveTgt.x, liveTgt.y, 9, 0, Math.PI * 2);
      ctx.stroke();
    }
    ctx.strokeStyle = pal.targetWhite;
    ctx.lineWidth = 2;
    ctx.beginPath();
    ctx.arc(tp.x, tp.y, 11, 0, Math.PI * 2);
    ctx.stroke();
    ctx.fillStyle = pal.targetWhite;
    ctx.beginPath();
    ctx.arc(tp.x, tp.y, 3, 0, Math.PI * 2);
    ctx.fill();
  });

  const pointerPosition = (event: ReactPointerEvent<HTMLCanvasElement>) => {
    const rect = event.currentTarget.getBoundingClientRect();
    const radius = Math.min(rect.width, rect.height) / 2 - 14;
    const dx = (event.clientX - rect.left - rect.width / 2) / radius;
    const dy = (event.clientY - rect.top - rect.height / 2) / radius;
    return diskPosition(dx, dy);
  };

  const apply = (p: { x: number; y: number }) => {
    draft.current.x = p.x;
    draft.current.y = p.y;
    position.set([p.x, p.y]);
  };

  return (
    <canvas
      ref={canvas}
      className="pilot-disk"
      style={{ width: size, height: size, cursor: 'crosshair', touchAction: 'none' }}
      onPointerDown={(event) => {
        event.currentTarget.setPointerCapture(event.pointerId);
        draft.current.dragging = true;
        apply(pointerPosition(event));
      }}
      onPointerMove={(event) => {
        if (draft.current.dragging) apply(pointerPosition(event));
      }}
      onPointerUp={(event) => {
        if (draft.current.dragging) apply(pointerPosition(event));
        draft.current.dragging = false;
      }}
      onDoubleClick={(event) => {
        apply(pointerPosition(event as unknown as ReactPointerEvent<HTMLCanvasElement>));
      }}
      aria-label="Pilot position disk"
    />
  );
}

// ------------------------------------------------------------------ axis dial

/**
 * One axis: 0 at the left, values increase clockwise, accent arc 0→value, quadrant labels,
 * live blue line, white target line + knob, 4-decimal value in the centre. Drag anywhere to
 * set; double-click for exact numeric entry.
 */
export function AxisDial({ axis, size = 150 }: { axis: 0 | 1; size?: number }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const axes = useParam<number[]>('/portal/pilot/axes');
  const ring = useRing('/tel/portal/selected');
  const [editing, setEditing] = useState<string | null>(null);
  const draft = useRef<{ dragging: boolean; value: number; other: number }>({
    dragging: false,
    value: 0,
    other: 0,
  });

  const value = axes.value;
  useEffect(() => {
    if (!draft.current.dragging && Array.isArray(value)) {
      draft.current.value = value[axis] ?? 0;
      draft.current.other = value[1 - axis] ?? 0;
    }
  }, [value, axis]);

  useRafDraw(() => {
    const el = canvas.current;
    if (!el) return;
    const ctx = fit(el);
    if (!ctx) return;
    const pal = palette();
    const { width: w, height: h } = el.getBoundingClientRect();
    const cx = w / 2;
    const cy = h / 2;
    const radius = Math.min(w, h) / 2 - 12;
    ctx.clearRect(0, 0, w, h);

    ctx.fillStyle = pal.surfaceTint;
    ctx.beginPath();
    ctx.arc(cx, cy, radius, 0, Math.PI * 2);
    ctx.fill();
    ctx.strokeStyle = pal.gridLine;
    ctx.lineWidth = 1.5;
    ctx.stroke();

    const v = draft.current.value;
    if (v > 0.001) {
      ctx.strokeStyle = pal.accent;
      ctx.globalAlpha = 0.65;
      ctx.lineWidth = 3;
      ctx.beginPath();
      ctx.arc(cx, cy, radius - 5, -Math.PI, dialAngle(v));
      ctx.stroke();
      ctx.globalAlpha = 1;
    }

    ctx.font = '9px var(--font-sans, sans-serif)';
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    for (const [quadrant, label] of [
      [0, '0'],
      [0.25, '.25'],
      [0.5, '.5'],
      [0.75, '.75'],
    ] as const) {
      const angle = dialAngle(quadrant);
      ctx.fillStyle = pal.gridLine;
      ctx.beginPath();
      ctx.arc(cx + Math.cos(angle) * radius, cy + Math.sin(angle) * radius, 2, 0, Math.PI * 2);
      ctx.fill();
      ctx.fillStyle = pal.textMuted;
      ctx.fillText(label, cx + Math.cos(angle) * radius * 1.22, cy + Math.sin(angle) * radius * 1.22);
    }

    const row = latestRow(ring);
    const live = row ? row[axis === 0 ? SEL.liveA : SEL.liveB] : Number.NaN;
    if (Number.isFinite(live)) {
      const angle = dialAngle(((live % 1) + 1) % 1);
      ctx.strokeStyle = pal.liveBlue;
      ctx.lineWidth = 3;
      ctx.beginPath();
      ctx.moveTo(cx, cy);
      ctx.lineTo(cx + Math.cos(angle) * radius * 0.88, cy + Math.sin(angle) * radius * 0.88);
      ctx.stroke();
    }

    const angle = dialAngle(((v % 1) + 1) % 1);
    const kx = cx + Math.cos(angle) * radius;
    const ky = cy + Math.sin(angle) * radius;
    ctx.strokeStyle = pal.targetWhite;
    ctx.lineWidth = 2;
    ctx.beginPath();
    ctx.moveTo(cx, cy);
    ctx.lineTo(kx, ky);
    ctx.stroke();
    ctx.fillStyle = pal.targetWhite;
    ctx.beginPath();
    ctx.arc(kx, ky, 4.5, 0, Math.PI * 2);
    ctx.fill();

    ctx.font = '12px var(--font-mono, monospace)';
    ctx.fillText(v.toFixed(4), cx, cy);
  });

  const setAxis = (v: number) => {
    draft.current.value = v;
    const pair: [number, number] = axis === 0 ? [v, draft.current.other] : [draft.current.other, v];
    axes.set(pair);
  };

  const pointerValue = (event: ReactPointerEvent<HTMLCanvasElement>) => {
    const rect = event.currentTarget.getBoundingClientRect();
    return dialValue(
      event.clientX - rect.left - rect.width / 2,
      event.clientY - rect.top - rect.height / 2,
    );
  };

  return (
    <span className="axis-dial-wrap">
      <canvas
        ref={canvas}
        className="axis-dial"
        style={{ width: size, height: size, cursor: 'pointer', touchAction: 'none' }}
        onPointerDown={(event) => {
          event.currentTarget.setPointerCapture(event.pointerId);
          draft.current.dragging = true;
          setAxis(pointerValue(event));
        }}
        onPointerMove={(event) => {
          if (draft.current.dragging) setAxis(pointerValue(event));
        }}
        onPointerUp={(event) => {
          if (draft.current.dragging) setAxis(pointerValue(event));
          draft.current.dragging = false;
        }}
        onDoubleClick={() => setEditing(draft.current.value.toFixed(4))}
        aria-label={`Axis ${axis === 0 ? 'A' : 'B'} dial`}
      />
      {editing !== null && (
        <span className="dial-edit">
          <input
            autoFocus
            value={editing}
            onChange={(event) => setEditing(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter') {
                const parsed = Number(editing);
                if (Number.isFinite(parsed)) setAxis(parsed);
                setEditing(null);
              }
              if (event.key === 'Escape') setEditing(null);
            }}
            onBlur={() => setEditing(null)}
            aria-label="Exact axis value"
          />
        </span>
      )}
    </span>
  );
}

// ------------------------------------------------------------------ pilot-all pad

/** Circle-clamped XY pad broadcasting a collateable move to a whole scope while dragged. */
export function PilotAllPad({ path, size = 140 }: { path: string; size?: number }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const param = useParam<number[]>(path);
  const draft = useRef<{ dragging: boolean; x: number; y: number }>({
    dragging: false,
    x: 0,
    y: 0,
  });

  const value = param.value;
  useEffect(() => {
    if (!draft.current.dragging && Array.isArray(value)) {
      draft.current.x = value[0] ?? 0;
      draft.current.y = value[1] ?? 0;
    }
  }, [value]);

  useRafDraw(() => {
    const el = canvas.current;
    if (!el) return;
    const ctx = fit(el);
    if (!ctx) return;
    const pal = palette();
    const { width: w, height: h } = el.getBoundingClientRect();
    const cx = w / 2;
    const cy = h / 2;
    const radius = Math.min(w, h) / 2 - 8;
    ctx.clearRect(0, 0, w, h);
    ctx.fillStyle = pal.surfaceTint;
    ctx.beginPath();
    ctx.arc(cx, cy, radius, 0, Math.PI * 2);
    ctx.fill();
    ctx.strokeStyle = pal.gridLine;
    ctx.lineWidth = 1;
    ctx.stroke();
    ctx.beginPath();
    ctx.moveTo(cx - radius, cy);
    ctx.lineTo(cx + radius, cy);
    ctx.moveTo(cx, cy - radius);
    ctx.lineTo(cx, cy + radius);
    ctx.stroke();
    ctx.fillStyle = pal.accent;
    ctx.beginPath();
    ctx.arc(cx + draft.current.x * radius, cy - draft.current.y * radius, 6, 0, Math.PI * 2);
    ctx.fill();
  });

  const apply = (event: ReactPointerEvent<HTMLCanvasElement>) => {
    const rect = event.currentTarget.getBoundingClientRect();
    const radius = Math.min(rect.width, rect.height) / 2 - 8;
    const clamped = clampToUnitCircle(
      (event.clientX - rect.left - rect.width / 2) / radius,
      -(event.clientY - rect.top - rect.height / 2) / radius,
    );
    draft.current.x = clamped.x;
    draft.current.y = clamped.y;
    param.set([clamped.x, clamped.y]);
  };

  return (
    <canvas
      ref={canvas}
      className="pilot-all-pad"
      style={{ width: size, height: size, cursor: 'crosshair', touchAction: 'none' }}
      onPointerDown={(event) => {
        event.currentTarget.setPointerCapture(event.pointerId);
        draft.current.dragging = true;
        apply(event);
      }}
      onPointerMove={(event) => {
        if (draft.current.dragging) apply(event);
      }}
      onPointerUp={(event) => {
        if (draft.current.dragging) apply(event);
        draft.current.dragging = false;
      }}
      aria-label="Pilot all portals"
    />
  );
}

// ------------------------------------------------------------------ image preview

/** The composited renderer output, nearest-neighbour, letterboxed to fit. */
export function ImagePreview({ height = 180 }: { height?: number }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const ring = useRing('/tel/preview');
  const [w, h] = useVec2('/installation/resolution');
  const imageRef = useRef<{
    data: ImageData | null;
    off: HTMLCanvasElement | null;
    w: number;
    h: number;
    seen: number;
  }>({ data: null, off: null, w: 0, h: 0, seen: -1 });

  useRafDraw(() => {
    const el = canvas.current;
    if (!el) return;
    const ctx = fit(el);
    if (!ctx) return;
    const rect = el.getBoundingClientRect();
    const iw = Math.max(1, Math.round(w));
    const ih = Math.max(1, Math.round(h));
    const cache = imageRef.current;
    if (!cache.data || cache.w !== iw || cache.h !== ih) {
      cache.data = new ImageData(iw, ih);
      cache.off = document.createElement('canvas');
      cache.off.width = iw;
      cache.off.height = ih;
      cache.w = iw;
      cache.h = ih;
      cache.seen = -1;
    }
    const row = latestRow(ring);
    if (row && ring && ring.writePos !== cache.seen && row.length >= iw * ih * 3) {
      const rgba = cache.data.data;
      for (let i = 0, j = 0; i < iw * ih; i++) {
        rgba[j++] = row[i * 3];
        rgba[j++] = row[i * 3 + 1];
        rgba[j++] = row[i * 3 + 2];
        rgba[j++] = 255;
      }
      cache.off?.getContext('2d')?.putImageData(cache.data, 0, 0);
      cache.seen = ring.writePos;
    }
    ctx.clearRect(0, 0, rect.width, rect.height);
    if (!cache.off) return;
    const scale = Math.min(rect.width / iw, rect.height / ih);
    const dw = iw * scale;
    const dh = ih * scale;
    const dx = (rect.width - dw) / 2;
    const dy = (rect.height - dh) / 2;
    ctx.imageSmoothingEnabled = false;
    ctx.strokeStyle = css('--router-grid-line', 'rgba(255,255,255,0.13)');
    ctx.strokeRect(dx - 0.5, dy - 0.5, dw + 1, dh + 1);
    ctx.drawImage(cache.off, dx, dy, dw, dh);
  });

  return (
    <canvas
      ref={canvas}
      className="image-preview"
      style={{ width: '100%', height, display: 'block' }}
      aria-label="Composited image preview"
    />
  );
}
