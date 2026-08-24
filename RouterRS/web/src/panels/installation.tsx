// The Installation tab: a slim broadcast toolbar, then the wall — the whole installation as
// one continuous grid of portal lenses, with each column's link state in a slim header strip
// spanning exactly its cells, and the pilot-all pad alongside. The columns are electrical
// groupings; the wall is the thing, so the wall is what the view draws.

import { useParam } from '@auroravision/av-gui/runtime';
import { useEffect, useRef, useState } from 'react';
import { Action, BroadcastActions, ColumnHeartbeats, OutboxChip } from '../bits';
import { PilotAllPad } from '../canvas';
import { InstallationGrid, WALL_SEP } from '../grid';
import { Target } from '../icons';
import { wallWidth } from '../math';
import { useBool, useNumber, useSelection } from '../model';

/** Room kept beside the wall for the pilot-all pad before the pad wraps below. */
const PAD_RESERVE = 190;

function ColumnHeader({ col }: { col: number }) {
  const selection = useSelection();
  const connected = useBool(`/columns/${col}/rs485/connected`);
  const selected =
    (selection.kind === 'column' || selection.kind === 'portal') && selection.col === col;
  return (
    <button
      type="button"
      className={`wall-header${selected ? ' is-selected' : ''}`}
      onClick={() => selection.selectColumn(col)}
      title={`Column ${col + 1} — ${connected ? 'bus connected' : 'bus down'}`}
    >
      <span className={`link-dot ${connected ? 'is-ok' : 'is-down'}`} />
      <span className="wall-header-name">{col + 1}</span>
      <ColumnHeartbeats col={col} />
      <OutboxChip col={col} quiet />
    </button>
  );
}

export function InstallationPanel() {
  const columns = Math.max(0, useNumber('/installation/arrangement/columns'));
  const rows = Math.max(1, useNumber('/installation/arrangement/rows'));
  const countX = Math.max(1, useNumber('/installation/arrangement/column_width'));
  const flipped = useBool('/installation/arrangement/flipped');
  const pilotAll = useParam<number[]>('/installation/pilot_all');

  // Cell size fills the available width (minus the pad's slot), clamped to stay a readable
  // dial; below the floor the wall scrolls horizontally instead of degrading.
  const wallRef = useRef<HTMLDivElement>(null);
  const [available, setAvailable] = useState(0);
  useEffect(() => {
    const el = wallRef.current;
    if (!el) return;
    const observer = new ResizeObserver((entries) => {
      const width = Math.floor(entries[0]?.contentRect.width ?? 0);
      setAvailable((previous) => (previous === width ? previous : width));
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, []);
  const totalAcross = Math.max(1, columns * countX);
  const separators = Math.max(0, columns - 1) * WALL_SEP;
  const cellPx = Math.max(
    28,
    Math.min(64, Math.floor((available - PAD_RESERVE - separators) / totalAcross)),
  );
  const gridWidth = wallWidth(columns, countX, cellPx, WALL_SEP);

  return (
    <div className="stack" data-av-surface="installation-map">
      <div className="installation-toolbar">
        <BroadcastActions prefix="/installation" />
        <span className="action-group" data-group="setup">
          <Action path="/installation/actions/home_and_zero">Home and zero local</Action>
          <Action path="/installation/actions/rebuild_columns">Rebuild columns</Action>
        </span>
      </div>

      <div className="wall" ref={wallRef}>
        <div className="wall-and-pad">
          <div className="wall-columns" style={{ width: gridWidth }}>
            <div className="wall-headers" data-av-surface="column-link">
              {Array.from({ length: columns }, (_, col) => (
                <div
                  key={col}
                  className="wall-header-slot"
                  style={{
                    width: countX * cellPx + (col < columns - 1 ? WALL_SEP : 0),
                    paddingRight: col < columns - 1 ? WALL_SEP : 0,
                  }}
                >
                  <ColumnHeader col={col} />
                </div>
              ))}
            </div>
            <InstallationGrid
              columns={columns}
              countX={countX}
              countY={rows}
              flipped={flipped}
              cellPx={cellPx}
            />
          </div>
          <div className="pilot-all">
            <span className="pilot-all-label">Pilot all</span>
            <PilotAllPad path="/installation/pilot_all" size={150} />
            <button
              type="button"
              className="quiet-button"
              onClick={() => pilotAll.set([0, 0])}
              title="Send every portal to centre"
            >
              <Target />
              centre
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
