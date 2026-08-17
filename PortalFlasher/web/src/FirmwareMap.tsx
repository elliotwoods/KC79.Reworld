/**
 * The firmware map: two lanes on one x-scale, spanning the whole 128 kB.
 *
 * SVG rather than canvas, deliberately. `constraints.md` §5 pushes anything above ~4 Hz onto a
 * canvas, but this redraws only when someone presses Read device — it is a static diagram, and as
 * SVG it can use `var(--token)` directly instead of resolving colours through `resolveColor` on
 * mount and resize.
 *
 * No `opacity`, `filter` or `mix-blend-mode` anywhere here: §4 forbids them on anything that
 * could paint over a viewport punch region, and dimming by colour token is the house rule.
 */

import { type MapModel, fillOf } from './firmware-map';

const LANE_HEIGHT = 26;
const LANE_GAP = 6;
const RULER_HEIGHT = 18;
const LABEL_WIDTH = 74;

export function FirmwareMap({ model }: { model: MapModel }) {
  if (model.lanes.length === 0) {
    return null;
  }

  const width = 1000; // viewBox units; the element scales to its container
  const plot = width - LABEL_WIDTH;
  const height = model.lanes.length * (LANE_HEIGHT + LANE_GAP) + RULER_HEIGHT;
  const splitX = LABEL_WIDTH + plot * model.splitFraction;

  return (
    <svg
      className="fw-map"
      viewBox={`0 0 ${width} ${height}`}
      preserveAspectRatio="none"
      role="img"
      aria-label="Flash contents, device compared with the selected image"
    >
      {model.lanes.map((lane, index) => {
        const y = index * (LANE_HEIGHT + LANE_GAP);
        const step = plot / lane.buckets.length;
        return (
          <g key={lane.label}>
            <text className="fw-map-label" x={0} y={y + LANE_HEIGHT * 0.7}>
              {lane.label}
            </text>
            <rect
              className="fw-map-track"
              x={LABEL_WIDTH}
              y={y}
              width={plot}
              height={LANE_HEIGHT}
            />
            {lane.buckets.map((bucket, i) => {
              const fill = fillOf(bucket);
              if (fill === 'erased') return null;
              // A bucket that differs from the other lane is drawn as a difference rather than as
              // content: which side has more bytes matters far less than that they disagree.
              const differs = model.diff?.[i] ?? false;
              return (
                <rect
                  key={i}
                  className="fw-map-cell"
                  data-fill={fill}
                  data-differs={differs ? 'yes' : 'no'}
                  x={LABEL_WIDTH + i * step}
                  y={y}
                  width={Math.max(step, 0.8)}
                  height={LANE_HEIGHT}
                />
              );
            })}
          </g>
        );
      })}

      {/* The bank boundary. The one line on this diagram that is a fact about the part rather
          than about the bytes. */}
      <line
        className="fw-map-split"
        x1={splitX}
        x2={splitX}
        y1={0}
        y2={model.lanes.length * (LANE_HEIGHT + LANE_GAP) - LANE_GAP}
      />

      <g transform={`translate(0, ${model.lanes.length * (LANE_HEIGHT + LANE_GAP)})`}>
        {model.ticks.map((tick) => {
          const x = LABEL_WIDTH + plot * tick.at;
          // The last label would overflow the viewBox if it stayed centred.
          const anchor = tick.at >= 1 ? 'end' : tick.at <= 0 ? 'start' : 'middle';
          return (
            <g key={tick.label}>
              <line className="fw-map-tick" x1={x} x2={x} y1={0} y2={5} />
              <text className="fw-map-tick-label" x={x} y={15} textAnchor={anchor}>
                {tick.label}
              </text>
            </g>
          );
        })}
      </g>
    </svg>
  );
}
