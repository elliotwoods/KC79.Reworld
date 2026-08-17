/**
 * The flash, drawn as what it physically is: a grid of 64 sectors of 2 kB.
 *
 * Plain DOM rather than SVG. The previous version was an SVG bar with
 * `preserveAspectRatio="none"`, which stretched its own labels into unreadable smears and gave a
 * texture instead of a picture. A grid of `<div>`s scales properly, gets hover titles and
 * accessible text for free, and — because a sector is the unit the flash algorithm erases — draws
 * exactly the resolution the hardware has and no more.
 *
 * Colour is by **bank**, so the bootloader and the application are visibly different things;
 * brightness is by how full a sector is; and a sector that disagrees with the selected image is
 * outlined rather than recoloured, because the disagreement is a fact *about* the sector rather
 * than a replacement for its contents.
 *
 * No `opacity`, `filter` or `mix-blend-mode`: `constraints.md` §4 forbids them on anything that
 * might paint over a viewport punch region, so every state is its own colour token.
 */

import type { MapModel } from './firmware-map';
import { rowAddress, sectorTitle } from './firmware-map';

export function FirmwareMap({ model }: { model: MapModel }) {
  if (model.sectors.length === 0) return null;

  const rows = Math.ceil(model.sectors.length / model.columns);
  const anyDiff = (model.diffCount ?? 0) > 0;

  return (
    <div className="fw-map">
      <div className="fw-grid" style={{ '--fw-columns': model.columns } as React.CSSProperties}>
        {Array.from({ length: rows }, (_, row) => (
          <div className="fw-row" key={row}>
            <span className="fw-row-addr">{rowAddress(model, row)}</span>
            <div className="fw-row-cells">
              {model.sectors
                .slice(row * model.columns, (row + 1) * model.columns)
                .map((sector) => (
                  <span
                    key={sector.index}
                    className="fw-cell"
                    data-region={sector.region}
                    data-fill={sector.device}
                    data-differs={sector.differs ? 'yes' : 'no'}
                    title={sectorTitle(sector, model.sectorBytes)}
                  />
                ))}
            </div>
          </div>
        ))}
      </div>

      {/* A key, because coloured blocks with no legend are decoration. */}
      <div className="fw-legend">
        <span className="fw-key">
          <span className="fw-cell fw-key-swatch" data-region="bootloader" data-fill="programmed" />
          bootloader · 24 kB
        </span>
        <span className="fw-key">
          <span
            className="fw-cell fw-key-swatch"
            data-region="application"
            data-fill="programmed"
          />
          application · 104 kB
        </span>
        <span className="fw-key">
          <span className="fw-cell fw-key-swatch" data-region="application" data-fill="erased" />
          erased
        </span>
        {anyDiff && (
          <span className="fw-key">
            <span
              className="fw-cell fw-key-swatch"
              data-region="application"
              data-fill="programmed"
              data-differs="yes"
            />
            differs from selected
          </span>
        )}
      </div>
    </div>
  );
}
