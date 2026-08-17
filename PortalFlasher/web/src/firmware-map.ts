/**
 * The firmware map's pure half.
 *
 * Everything that decides what the map *says* is a function of plain arrays, tested without a
 * bus, without jsdom and without a board — so the component is left with nothing but geometry
 * and markup. Same discipline as the framework's `web/src/vision/devices.ts`.
 *
 * # Why two lanes
 *
 * One lane tells you a board has firmware on it. Two lanes, on the same x-scale, tell you whether
 * it is *the firmware you selected* — and where it differs. That comparison is the entire reason
 * the map exists; a single lane would be decoration.
 */

/** How full one slice of flash is, 0..=255, as the Rust side buckets it. */
export type Bucket = number;

export type Fill = 'erased' | 'partial' | 'programmed';

/** The three states a slice can be in. Anything not fully erased is worth showing as *something*
 * — a single programmed byte in an otherwise blank sector is exactly the kind of thing an
 * operator wants to see rather than have rounded away. */
export function fillOf(bucket: Bucket): Fill {
  if (bucket <= 0) return 'erased';
  if (bucket >= 250) return 'programmed';
  return 'partial';
}

export interface Lane {
  /** Shown at the left of the lane. */
  label: string;
  buckets: Bucket[];
  /** True when this lane is what the operator selected rather than what the board holds. */
  selected: boolean;
}

export interface Tick {
  /** 0..1 across the whole flash. */
  at: number;
  label: string;
}

export interface MapModel {
  lanes: Lane[];
  /** Where the bootloader bank ends, 0..1. */
  splitFraction: number;
  ticks: Tick[];
  /**
   * Per bucket: does the device disagree with the selected image? `null` when there is nothing
   * to compare against, which the page draws as one lane rather than as agreement.
   */
  diff: boolean[] | null;
  /** How many buckets disagree. 0 with a non-null `diff` means "this board already matches". */
  diffCount: number;
}

/** Byte offsets worth a label on a 128 kB part. */
const TICK_BYTES: ReadonlyArray<readonly [number, string]> = [
  [0, '0'],
  [24 * 1024, '24k'],
  [64 * 1024, '64k'],
  [128 * 1024, '128k'],
];

const FLASH_BYTES = 128 * 1024;

export function ticks(): Tick[] {
  return TICK_BYTES.map(([at, label]) => ({ at: at / FLASH_BYTES, label }));
}

/**
 * Where the two lanes disagree.
 *
 * Compared by fill class rather than by exact value, because the buckets are lossy: a sector that
 * is 3/512 programmed on one side and 4/512 on the other is not a difference an operator can act
 * on, and flagging it would make every map look wrong.
 */
export function diffBuckets(device: Bucket[], selected: Bucket[]): boolean[] {
  const length = Math.min(device.length, selected.length);
  const out: boolean[] = [];
  for (let i = 0; i < length; i++) {
    out.push(fillOf(device[i]) !== fillOf(selected[i]));
  }
  return out;
}

export function buildMap(
  device: Bucket[] | null,
  selected: Bucket[] | null,
  splitFraction: number,
): MapModel {
  const lanes: Lane[] = [];
  if (device) lanes.push({ label: 'on device', buckets: device, selected: false });
  if (selected) lanes.push({ label: 'selected', buckets: selected, selected: true });

  const diff = device && selected ? diffBuckets(device, selected) : null;
  return {
    lanes,
    splitFraction,
    ticks: ticks(),
    diff,
    diffCount: diff ? diff.filter(Boolean).length : 0,
  };
}

/** A one-line verdict for the panel heading. */
export function comparisonSummary(model: MapModel): { label: string; tone: 'ok' | 'warn' | 'idle' } {
  if (model.lanes.length === 0) return { label: 'not read', tone: 'idle' };
  if (!model.diff) return { label: 'no image selected', tone: 'idle' };
  if (model.diffCount === 0) return { label: 'matches the selected image', tone: 'ok' };
  const percent = Math.max(1, Math.round((100 * model.diffCount) / model.diff.length));
  return { label: `differs across ~${percent}% of flash`, tone: 'warn' };
}

/** Human byte counts, for the region rows. */
export function formatBytes(bytes: number): string {
  if (bytes <= 0) return '0';
  if (bytes < 1024) return `${bytes} B`;
  return `${(bytes / 1024).toFixed(1)} kB`;
}
