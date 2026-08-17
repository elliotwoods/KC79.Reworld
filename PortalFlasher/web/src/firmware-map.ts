/**
 * The firmware map's pure half.
 *
 * Everything that decides what the map *says* is a function of plain arrays, tested without a bus,
 * without jsdom and without a board — so the component is left with nothing but geometry and
 * markup. Same discipline as the framework's `web/src/vision/devices.ts`.
 *
 * # A grid of sectors, not a stripe
 *
 * The first version drew two horizontal bars of 256 arbitrary buckets. It read as a texture: you
 * could see that *something* was programmed but not what, where, or in what units, and the labels
 * were stretched by a non-uniform aspect ratio.
 *
 * This draws the flash as what it physically is — **64 sectors of 2 kB**, the unit the STM32G0's
 * flash algorithm actually erases. Twelve of them are the bootloader bank; the rest are the
 * application. A sector is therefore the finest granularity at which two images can genuinely
 * differ, so nothing here is invented resolution.
 */

/** How full one sector is, 0..=255, as the Rust side reports it. */
export type Occupancy = number;

export type Fill = 'erased' | 'partial' | 'programmed';

export type Region = 'bootloader' | 'application';

/**
 * The three states a sector can be in.
 *
 * Anything not fully erased shows as *something*: a single programmed byte in an otherwise blank
 * sector is exactly the kind of thing an operator wants to see rather than have rounded away.
 */
export function fillOf(value: Occupancy): Fill {
  if (value <= 0) return 'erased';
  if (value >= 250) return 'programmed';
  return 'partial';
}

export interface Sector {
  index: number;
  /** Start address, for the row labels and the tooltip. */
  address: number;
  region: Region;
  device: Fill;
  /** What the selected image would put here, or `null` when nothing is selected. */
  selected: Fill | null;
  /** True when the board and the selected image disagree about this sector. */
  differs: boolean;
}

export interface MapModel {
  sectors: Sector[];
  /** Sectors per row in the grid. */
  columns: number;
  sectorBytes: number;
  flashBase: number;
  /** How many sectors are drawn but have no data — always 0 in practice; a guard for a short
   * occupancy array rather than a silent short grid. */
  missing: number;
  /** `null` when nothing is selected: the map then shows the board without implying agreement. */
  diffCount: number | null;
}

/** Eight across gives eight rows of 16 kB — square enough to scan, and every row boundary lands
 * on a round address. */
export const COLUMNS = 8;

export function buildMap(
  device: Occupancy[] | null,
  selected: Occupancy[] | null,
  options: { sectorBytes: number; bootloaderSectors: number; flashBase: number },
): MapModel {
  if (!device || device.length === 0) {
    return {
      sectors: [],
      columns: COLUMNS,
      sectorBytes: options.sectorBytes,
      flashBase: options.flashBase,
      missing: 0,
      diffCount: null,
    };
  }

  const sectors: Sector[] = device.map((value, index) => {
    const deviceFill = fillOf(value);
    const selectedFill = selected && index < selected.length ? fillOf(selected[index]) : null;
    return {
      index,
      address: options.flashBase + index * options.sectorBytes,
      region: index < options.bootloaderSectors ? 'bootloader' : 'application',
      device: deviceFill,
      selected: selectedFill,
      // Compared by fill class rather than by exact value: the occupancy is a fraction, and a
      // sector that is 3/2048 programmed on one side and 4/2048 on the other is not a difference
      // anyone can act on. Flagging it would make every map look wrong.
      differs: selectedFill !== null && selectedFill !== deviceFill,
    };
  });

  return {
    sectors,
    columns: COLUMNS,
    sectorBytes: options.sectorBytes,
    flashBase: options.flashBase,
    missing: selected ? Math.max(0, device.length - selected.length) : 0,
    diffCount: selected ? sectors.filter((s) => s.differs).length : null,
  };
}

/** How many sectors of a region are not erased. */
export function usedSectors(model: MapModel, region: Region): number {
  return model.sectors.filter((s) => s.region === region && s.device !== 'erased').length;
}

export function regionSectors(model: MapModel, region: Region): number {
  return model.sectors.filter((s) => s.region === region).length;
}

/** A one-line verdict about the board against the selected image. */
export function comparisonSummary(
  model: MapModel,
): { label: string; tone: 'ok' | 'warn' | 'idle' } | null {
  if (model.sectors.length === 0) return null;
  // No selection is not a verdict. Saying "no image selected" *here* read as a complaint about
  // the flash contents rather than about the firmware picker, so the panel says nothing instead.
  if (model.diffCount === null) return null;
  if (model.diffCount === 0) return { label: 'matches the selected image', tone: 'ok' };
  const label =
    model.diffCount === 1
      ? '1 sector differs from the selected image'
      : `${model.diffCount} sectors differ from the selected image`;
  return { label, tone: 'warn' };
}

/** Row label: the address the row starts at. */
export function rowAddress(model: MapModel, row: number): string {
  const address = model.flashBase + row * model.columns * model.sectorBytes;
  return `0x${address.toString(16).toUpperCase().padStart(8, '0')}`;
}

/** What one sector should say when hovered. */
export function sectorTitle(sector: Sector, sectorBytes: number): string {
  const address = `0x${sector.address.toString(16).toUpperCase().padStart(8, '0')}`;
  const size = `${sectorBytes / 1024} kB`;
  const state = sector.device === 'erased' ? 'erased' : sector.device;
  const diff = sector.differs ? ' · differs from the selected image' : '';
  return `${address} · sector ${sector.index} · ${size} · ${sector.region} · ${state}${diff}`;
}

/** Human byte counts, for the region rows. */
export function formatBytes(bytes: number): string {
  if (bytes <= 0) return '0';
  if (bytes < 1024) return `${bytes} B`;
  return `${(bytes / 1024).toFixed(1)} kB`;
}
