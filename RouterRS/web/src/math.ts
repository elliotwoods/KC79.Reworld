// Pure geometry shared by the canvas components, ported 1:1 from the iced widgets
// (crates/router-app/src/widgets.rs), which themselves port the C++ ofxCvGui panels.
// Exported un-DOM-coupled so vitest can pin the numbers.

/** The C++ disk warps radius by pow(r, 0.4) for finer centre control. */
export const DISK_WARP = 0.4;
/** Heartbeat dots fade linearly over 300 ms. */
export const HEARTBEAT_FADE_MS = 300;

export const heartbeatAlpha = (ageMs: number): number =>
  Number.isFinite(ageMs) ? Math.min(1, Math.max(0, 1 - ageMs / HEARTBEAT_FADE_MS)) : 0;

/** Installation position -> disk screen offset in units of the disk radius (screen y down). */
export function diskPoint(x: number, y: number): { dx: number; dy: number } {
  const r = Math.hypot(x, y);
  const warped = r > 0 ? r ** DISK_WARP : 0;
  const scale = r > 0 ? warped / r : 0;
  return { dx: x * scale, dy: -y * scale };
}

/** Disk pointer offset (units of radius, screen y down) -> installation position. */
export function diskPosition(dx: number, dy: number): { x: number; y: number } {
  const screenR = Math.hypot(dx, dy);
  const r = Math.min(screenR, 1) ** (1 / DISK_WARP);
  const scale = screenR > 0 ? r / screenR : 0;
  return { x: dx * scale, y: -dy * scale };
}

/** Axis dial: value 0..1 -> angle in radians (0 = left, clockwise on screen). */
export const dialAngle = (value: number): number => value * Math.PI * 2 - Math.PI;

/** Axis dial: pointer offset from centre (screen y down) -> value 0..1. */
export function dialValue(dx: number, dy: number): number {
  const angle = Math.atan2(dy, dx);
  const value = (angle + Math.PI) / (Math.PI * 2);
  return ((value % 1) + 1) % 1;
}

/**
 * Portal grid cell for the portal at `index` (0-based vector order, target = index + 1).
 * When NOT flipped, portal 1 sits at the BOTTOM (image rows run bottom-to-top).
 */
export function gridCell(
  index: number,
  countX: number,
  countY: number,
  flipped: boolean,
): { gx: number; gy: number } {
  const gx = index % countX;
  const row = Math.floor(index / countX);
  const gy = flipped ? row : countY - 1 - row;
  return { gx, gy };
}

/** Inverse hit test: pixel -> portal index, or -1 outside every cell. */
export function gridHit(
  px: number,
  py: number,
  width: number,
  height: number,
  countX: number,
  countY: number,
  flipped: boolean,
  count: number,
): number {
  const gx = Math.floor((px / width) * countX);
  const gy = Math.floor((py / height) * countY);
  if (gx < 0 || gx >= countX || gy < 0 || gy >= countY) return -1;
  const row = flipped ? gy : countY - 1 - gy;
  const index = row * countX + gx;
  return index >= 0 && index < count ? index : -1;
}

/** Telemetry slot for (column, target id) given the published slot offsets. */
export const slotOf = (offsets: number[], col: number, target: number): number =>
  (offsets[col] ?? 0) + (target - 1);

// ------------------------------------------------------------------ the wall
//
// The installation is one continuous grid of portals; columns are electrical groupings, not
// visual boxes. These helpers lay all columns out on one canvas with a thin separator at each
// column boundary, and are pure so vitest can pin the pixel math.

/** Pixel x of a column's left edge on the wall canvas. */
export const wallColumnX = (col: number, countX: number, cell: number, sep: number): number =>
  col * (countX * cell + sep);

/** Total wall canvas width. */
export const wallWidth = (columns: number, countX: number, cell: number, sep: number): number =>
  columns * countX * cell + Math.max(0, columns - 1) * sep;

/**
 * Pixel -> portal, across column separators. Returns the 0-based portal index (target − 1)
 * or null in a separator / outside the wall. Same bottom-up row rule as [`gridCell`].
 */
export function wallHit(
  px: number,
  py: number,
  columns: number,
  countX: number,
  countY: number,
  flipped: boolean,
  cell: number,
  sep: number,
): { col: number; index: number } | null {
  if (px < 0 || py < 0 || py >= countY * cell) return null;
  const span = countX * cell + sep;
  const col = Math.floor(px / span);
  if (col < 0 || col >= columns) return null;
  const localX = px - col * span;
  if (localX >= countX * cell) return null; // in the separator
  const gx = Math.floor(localX / cell);
  const gy = Math.floor(py / cell);
  const row = flipped ? gy : countY - 1 - gy;
  const index = row * countX + gx;
  return index >= 0 && index < countX * countY ? { col, index } : null;
}

/**
 * The preview pixel a portal samples, as an index into the W×H image — mirroring
 * `Column::update_positions_from_image` exactly: `x = col·countX + i%countX`,
 * `y = flipped ? row : countY−1−row`.
 */
export function previewIndex(
  col: number,
  index: number,
  countX: number,
  countY: number,
  flipped: boolean,
  width: number,
  height: number,
): number | null {
  const x = col * countX + (index % countX);
  const row = Math.floor(index / countX);
  const y = flipped ? row : countY - 1 - row;
  if (x < 0 || x >= width || y < 0 || y >= height) return null;
  return y * width + x;
}

/** Clamp a point to the unit circle (the pilot-all pads and the REST setPosition rule). */
export function clampToUnitCircle(x: number, y: number): { x: number; y: number } {
  const r = Math.hypot(x, y);
  return r > 1 ? { x: x / r, y: y / r } : { x, y };
}

/** `Xd Xh Xm Xs` uptime, matching `Utils::millisToString`. */
export function formatUptime(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) return '—';
  const s = Math.floor(ms / 1000);
  const days = Math.floor(s / 86400);
  const hours = Math.floor((s % 86400) / 3600);
  const minutes = Math.floor((s % 3600) / 60);
  const seconds = s % 60;
  const parts: string[] = [];
  if (days) parts.push(`${days}d`);
  if (days || hours) parts.push(`${hours}h`);
  if (days || hours || minutes) parts.push(`${minutes}m`);
  parts.push(`${seconds}s`);
  return parts.join(' ');
}

export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return '—';
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} kB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
