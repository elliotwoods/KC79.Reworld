// The wall: the whole installation as ONE continuous grid of portal lenses on a single
// canvas (columns are electrical groupings, drawn as hairline separators, not boxes), plus
// the HealthHeatmap on the Diagnostics tab. Both read the wide telemetry channels by slot
// index and hit-test clicks into `/ui/select/*` writes.

import { useRef } from 'react';
import { fit, palette, useRafDraw } from './canvas';
import {
  gridCell,
  heartbeatAlpha,
  gridHit,
  previewIndex,
  wallColumnX,
  wallHit,
  wallWidth,
} from './math';
import { latestRow, useBool, useRing, useSelection, useSlotOffsets } from './model';

/** Health-state cell tints (state index: 0 unknown, 1 ok, 2 degraded, 3 faulty, 4 silent). */
function healthFill(state: number, pal: ReturnType<typeof palette>): string | null {
  switch (state) {
    case 1:
      return withAlpha(pal.ok, 0.07);
    case 2:
      return withAlpha(pal.warn, 0.16);
    case 3:
      return withAlpha(pal.error, 0.2);
    case 4:
      return withAlpha(pal.error, 0.3);
    default:
      return null;
  }
}

export function withAlpha(color: string, alpha: number): string {
  // Token colors are hex (#rrggbb) or rgb()/rgba(); normalise to rgba.
  if (color.startsWith('#') && (color.length === 7 || color.length === 4)) {
    const hex =
      color.length === 4
        ? `#${color[1]}${color[1]}${color[2]}${color[2]}${color[3]}${color[3]}`
        : color;
    const r = parseInt(hex.slice(1, 3), 16);
    const g = parseInt(hex.slice(3, 5), 16);
    const b = parseInt(hex.slice(5, 7), 16);
    return `rgba(${r},${g},${b},${alpha})`;
  }
  const match = color.match(/rgba?\(([^)]+)\)/);
  if (match) {
    const [r, g, b] = match[1].split(',').map((v) => v.trim());
    return `rgba(${r},${g},${b},${alpha})`;
  }
  return color;
}

/** Separator between column spans, in px — a hairline plus breathing room, not a gap. */
export const WALL_SEP = 6;

export interface WallProps {
  columns: number;
  countX: number;
  countY: number;
  flipped: boolean;
  /** Cell size in px, computed by the parent to fill the available width. */
  cellPx: number;
}

/**
 * The installation as it is on the wall: one continuous grid of portal lenses. Each cell is a
 * miniature instrument dial — aperture ring, crosshair ticks, white target ring, blue live
 * dot, leader while they disagree, rx/tx rim arcs fading over 300 ms — and, when image
 * sampling drives the wall, a wash of the exact preview pixel this portal samples, so the
 * Renderer and the wall visibly agree. Click a cell to inspect that portal.
 */
export function InstallationGrid({ columns, countX, countY, flipped, cellPx }: WallProps) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const pose = useRing('/tel/portals/pose');
  const link = useRing('/tel/portals/link');
  const preview = useRing('/tel/preview');
  const imageEnabled = useBool('/installation/image_enabled');
  const offsets = useSlotOffsets();
  const selection = useSelection();
  const selected =
    selection.kind === 'portal' ? { col: selection.col, index: selection.portal - 1 } : null;
  const width = wallWidth(columns, countX, cellPx, WALL_SEP);
  const height = countY * cellPx;
  const previewW = columns * countX;

  const hover = useRef<{ col: number; index: number } | null>(null);
  const arrival = useRef<{ writePos: number; at: number }>({ writePos: -1, at: 0 });

  useRafDraw(() => {
    const el = canvas.current;
    if (!el) return;
    const ctx = fit(el);
    if (!ctx) return;
    const pal = palette();
    ctx.clearRect(0, 0, width, height);
    const poseRow = latestRow(pose);
    const linkRow = latestRow(link);
    const previewRow = imageEnabled ? latestRow(preview) : null;
    if (link && arrival.current.writePos !== link.writePos) {
      arrival.current = { writePos: link.writePos, at: performance.now() };
    }
    const ageExtra = link ? performance.now() - arrival.current.at : 0;

    // Marker sizes breathe with the cell, capped so a big wall stays a dial, not a target.
    const s = Math.min(Math.max(cellPx / 30, 1), 1.8);
    const rTarget = Math.min(3.5 * s, 7);
    const rLive = Math.min(2.5 * s, 5);

    for (let col = 0; col < columns; col++) {
      const colX = wallColumnX(col, countX, cellPx, WALL_SEP);

      // Hairline separator after every column but the last.
      if (col < columns - 1) {
        ctx.strokeStyle = pal.gridLine;
        ctx.lineWidth = 1;
        ctx.beginPath();
        ctx.moveTo(colX + countX * cellPx + WALL_SEP / 2, 2);
        ctx.lineTo(colX + countX * cellPx + WALL_SEP / 2, height - 2);
        ctx.stroke();
      }

      const base = offsets[col] ?? 0;
      for (let i = 0; i < countX * countY; i++) {
        const { gx, gy } = gridCell(i, countX, countY, flipped);
        const x = colX + gx * cellPx;
        const y = gy * cellPx;
        const ccx = x + cellPx / 2;
        const ccy = y + cellPx / 2;
        const apr = cellPx / 2 - 3;
        const slot = base + i;
        const isSelected = selected != null && selected.col === col && selected.index === i;
        const isHovered =
          hover.current != null && hover.current.col === col && hover.current.index === i;
        const state = linkRow ? linkRow[slot * 4 + 2] : 0;

        // Ground wash: a sick unit is never hidden by the image, so health wins.
        let wash: string | null = null;
        if (state >= 2) {
          wash = healthFill(state, pal);
        } else {
          if (previewRow) {
            const pixel = previewIndex(col, i, countX, countY, flipped, previewW, countY);
            if (pixel != null && previewRow.length >= (pixel + 1) * 3) {
              const r = Math.round(previewRow[pixel * 3]);
              const g = Math.round(previewRow[pixel * 3 + 1]);
              const b = Math.round(previewRow[pixel * 3 + 2]);
              wash = `rgba(${r},${g},${b},0.22)`;
            }
          }
          if (wash == null && state === 1) wash = healthFill(state, pal);
        }
        if (wash) {
          ctx.fillStyle = wash;
          ctx.beginPath();
          ctx.roundRect(x + 1.5, y + 1.5, cellPx - 3, cellPx - 3, Math.min(6, cellPx / 5));
          ctx.fill();
        }

        // Selection: an accent ring around the whole lens.
        if (isSelected) {
          ctx.strokeStyle = pal.accent;
          ctx.lineWidth = 1.5;
          ctx.beginPath();
          ctx.arc(ccx, ccy, apr + 1.5, 0, Math.PI * 2);
          ctx.stroke();
        }

        // The aperture: a faint lens ring, a fainter mid ring, crosshair ticks at the rim.
        ctx.strokeStyle = isHovered ? pal.textMuted : pal.gridLine;
        ctx.lineWidth = 1;
        ctx.beginPath();
        ctx.arc(ccx, ccy, apr, 0, Math.PI * 2);
        ctx.stroke();
        ctx.strokeStyle = withAlpha(pal.targetWhite, isHovered ? 0.16 : 0.08);
        ctx.beginPath();
        ctx.arc(ccx, ccy, apr * 0.5, 0, Math.PI * 2);
        ctx.stroke();
        const tick = Math.min(3 * s, 5);
        ctx.strokeStyle = pal.gridLine;
        ctx.beginPath();
        for (const [dx, dy] of [
          [0, -1],
          [1, 0],
          [0, 1],
          [-1, 0],
        ]) {
          ctx.moveTo(ccx + dx * (apr - tick), ccy + dy * (apr - tick));
          ctx.lineTo(ccx + dx * apr, ccy + dy * apr);
        }
        ctx.stroke();

        // Positions, unit-clamped into the aperture (markers stay inside the lens).
        const radius = apr - rTarget - 1;
        const toPoint = (px: number, py: number) => {
          const r = Math.hypot(px, py);
          const clamped = Math.min(r, 1);
          const scale = r > 0 ? clamped / r : 0;
          return { x: ccx + px * scale * radius, y: ccy - py * scale * radius };
        };
        if (poseRow) {
          const o = slot * 4;
          const tp = toPoint(poseRow[o], poseRow[o + 1]);
          if (Number.isFinite(poseRow[o + 2])) {
            const lp = toPoint(poseRow[o + 2], poseRow[o + 3]);
            if (Math.hypot(lp.x - tp.x, lp.y - tp.y) > 3) {
              ctx.strokeStyle = 'rgba(255,255,255,0.25)';
              ctx.lineWidth = 1;
              ctx.beginPath();
              ctx.moveTo(lp.x, lp.y);
              ctx.lineTo(tp.x, tp.y);
              ctx.stroke();
            }
            ctx.fillStyle = pal.liveBlue;
            ctx.beginPath();
            ctx.arc(lp.x, lp.y, rLive, 0, Math.PI * 2);
            ctx.fill();
          }
          ctx.strokeStyle = pal.targetWhite;
          ctx.lineWidth = 1.5;
          ctx.beginPath();
          ctx.arc(tp.x, tp.y, rTarget, 0, Math.PI * 2);
          ctx.stroke();
        }

        // rx (green, upper-left) and tx (accent, upper-right) as rim arcs on the lens.
        if (linkRow) {
          const rxAlpha = heartbeatAlpha(linkRow[slot * 4] + ageExtra);
          if (rxAlpha > 0) {
            ctx.strokeStyle = withAlpha(pal.ok, rxAlpha);
            ctx.lineWidth = 2;
            ctx.beginPath();
            ctx.arc(ccx, ccy, apr, (-3 * Math.PI) / 4 - 0.35, (-3 * Math.PI) / 4 + 0.35);
            ctx.stroke();
          }
          const txAlpha = heartbeatAlpha(linkRow[slot * 4 + 1] + ageExtra);
          if (txAlpha > 0) {
            ctx.strokeStyle = withAlpha(pal.accent, txAlpha);
            ctx.lineWidth = 2;
            ctx.beginPath();
            ctx.arc(ccx, ccy, apr, -Math.PI / 4 - 0.35, -Math.PI / 4 + 0.35);
            ctx.stroke();
          }
        }

        // Target id, tucked in the corner outside the lens.
        if (cellPx >= 26) {
          ctx.fillStyle = isHovered ? pal.textMuted : 'rgba(255,255,255,0.3)';
          ctx.font = `${cellPx >= 48 ? 10 : 9}px var(--font-sans, sans-serif)`;
          ctx.textAlign = 'left';
          ctx.textBaseline = 'top';
          ctx.fillText(String(i + 1), x + 3, y + 2);
        }
      }
    }
  });

  const hit = (event: React.PointerEvent<HTMLCanvasElement>) => {
    const rect = event.currentTarget.getBoundingClientRect();
    return wallHit(
      event.clientX - rect.left,
      event.clientY - rect.top,
      columns,
      countX,
      countY,
      flipped,
      cellPx,
      WALL_SEP,
    );
  };

  return (
    <canvas
      ref={canvas}
      className="wall-grid"
      style={{ width, height, cursor: 'pointer', touchAction: 'none' }}
      onPointerMove={(event) => {
        hover.current = hit(event);
      }}
      onPointerLeave={() => {
        hover.current = null;
      }}
      onPointerDown={(event) => {
        const cell = hit(event);
        if (cell) selection.selectPortal(cell.col, cell.index + 1);
      }}
      aria-label="Installation wall: every portal's target and live position"
    />
  );
}

export interface HeatColumnShape {
  countX: number;
  countY: number;
  flipped: boolean;
}

/**
 * Installation-shaped health heatmap: every column side by side with 8 px gaps, cells
 * coloured by health state (Ok alpha scales with score), click to inspect the portal.
 */
export function HealthHeatmap({ columns }: { columns: HeatColumnShape[] }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const link = useRing('/tel/portals/link');
  const offsets = useSlotOffsets();
  const selection = useSelection();
  const GAP = 8;
  const maxCountY = Math.max(1, ...columns.map((c) => c.countY));
  const height = Math.max(120, maxCountY * 9);

  useRafDraw(() => {
    const el = canvas.current;
    if (!el) return;
    const ctx = fit(el);
    if (!ctx) return;
    const pal = palette();
    const rect = el.getBoundingClientRect();
    ctx.clearRect(0, 0, rect.width, rect.height);
    if (columns.length === 0) return;
    const linkRow = latestRow(link);
    const colWidth = (rect.width - GAP * (columns.length - 1)) / columns.length;

    const stateColor = (state: number, score: number): string => {
      switch (state) {
        case 1:
          return withAlpha(pal.ok, 0.25 + 0.45 * (score / 100));
        case 2:
          return withAlpha(pal.warn, 0.85);
        case 3:
          return withAlpha(pal.error, 0.85);
        case 4:
          return withAlpha(pal.error, 0.85);
        default:
          return withAlpha(pal.textMuted, 0.25);
      }
    };

    columns.forEach((column, colIndex) => {
      const areaX = colIndex * (colWidth + GAP);
      const cw = colWidth / column.countX;
      const ch = rect.height / column.countY;
      const base = offsets[colIndex] ?? 0;
      for (let i = 0; i < column.countX * column.countY; i++) {
        const { gx, gy } = gridCell(i, column.countX, column.countY, column.flipped);
        const slot = base + i;
        const state = linkRow ? linkRow[slot * 4 + 2] : 0;
        const score = linkRow ? linkRow[slot * 4 + 3] : 0;
        ctx.fillStyle = stateColor(state, score);
        ctx.fillRect(
          areaX + gx * cw + 1,
          gy * ch + 1,
          Math.max(1, cw - 2),
          Math.max(1, ch - 2),
        );
      }
      ctx.strokeStyle = pal.gridLine;
      ctx.lineWidth = 0.5;
      ctx.strokeRect(areaX, 0, colWidth, rect.height);
    });
  });

  return (
    <canvas
      ref={canvas}
      className="health-heatmap"
      style={{ width: '100%', height, cursor: 'pointer', touchAction: 'none' }}
      onPointerDown={(event) => {
        const rect = event.currentTarget.getBoundingClientRect();
        if (columns.length === 0) return;
        const GAP_ = 8;
        const colWidth = (rect.width - GAP_ * (columns.length - 1)) / columns.length;
        const px = event.clientX - rect.left;
        const colIndex = Math.min(
          columns.length - 1,
          Math.max(0, Math.floor(px / (colWidth + GAP_))),
        );
        const column = columns[colIndex];
        const localX = px - colIndex * (colWidth + GAP_);
        const index = gridHit(
          localX,
          event.clientY - rect.top,
          colWidth,
          rect.height,
          column.countX,
          column.countY,
          column.flipped,
          column.countX * column.countY,
        );
        if (index >= 0) selection.selectPortal(colIndex, index + 1);
      }}
      aria-label="Installation health heatmap"
    />
  );
}
