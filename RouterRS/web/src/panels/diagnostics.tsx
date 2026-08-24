// The Diagnostics tab: stat tiles, the installation health heatmap, per-connection and
// worst-unit tables, the fault feed, and the session report controls. Tables and the feed
// are documents — polled from `/api/router/diagnostics` at 1 Hz; live cells (heatmap) ride
// telemetry.

import { Badge, Panel, Row, TextField, Toggle } from '@auroravision/av-gui/controls';
import { useEffect, useState, type ReactNode } from 'react';
import { Action, Fact } from '../bits';
import { HealthHeatmap } from '../grid';
import {
  Activity,
  AlertTriangle,
  Bug,
  Cable,
  Clock,
  FileText,
  iconForFault,
  Layers,
  ScrollText,
  Send,
  type IconComponent,
} from '../icons';
import { formatBytes } from '../math';
import { api, useNumber, useSelection, useText } from '../model';

interface Diag {
  session_file: string;
  file_bytes: number;
  dropped_events: number;
  verbose: boolean;
  columns: {
    col: number;
    state: string;
    endpoint: string;
    tx: number;
    rx: number;
    timeouts: number;
    cobs_errors: number;
    msgpack_errors: number;
    latency_p50_ms: number;
    latency_p90_ms: number;
    latency_p99_ms: number;
  }[];
  portals: {
    col: number;
    portal: number;
    state: string;
    score: number;
    ack_rate: number;
    timeouts: number;
    error_logs: number;
    last_seen_age_ms: number | null;
  }[];
  recent_faults: {
    ts_ms: number;
    kind: string;
    col: number;
    portal: number | null;
    detail: string;
    repeat: number;
  }[];
}

function useDiagnostics(): Diag | null {
  const [diag, setDiag] = useState<Diag | null>(null);
  useEffect(() => {
    let alive = true;
    const poll = () => {
      api<Diag>('/api/router/diagnostics')
        .then((doc) => {
          if (alive) setDiag(doc);
        })
        .catch(() => {});
    };
    poll();
    const timer = setInterval(poll, 1000);
    return () => {
      alive = false;
      clearInterval(timer);
    };
  }, []);
  return diag;
}

const stateTone = (state: string): 'ok' | 'warn' | 'error' | 'idle' =>
  state === 'ok' || state === 'connected'
    ? 'ok'
    : state === 'degraded' || state === 'stalled' || state === 'noisy'
      ? 'warn'
      : state === 'faulty' || state === 'silent'
        ? 'error'
        : 'idle';

function Tile({
  label,
  value,
  tone,
  icon: Glyph,
}: {
  label: string;
  value: ReactNode;
  tone?: 'error';
  icon: IconComponent;
}) {
  return (
    <div className={`stat-tile-local${tone ? ` is-${tone}` : ''}`}>
      <div className="stat-tile-local-label">
        <Glyph />
        {label}
      </div>
      <div className="stat-tile-local-value">{value}</div>
    </div>
  );
}

export function DiagnosticsPanel() {
  const diag = useDiagnostics();
  const selection = useSelection();
  const txRate = useNumber('/stats/tx_per_s');
  const rxRate = useNumber('/stats/rx_per_s');
  const faulty = useNumber('/health/faulty_units');
  const sessionFile = useText('/report/session_file');
  const fileBytes = useNumber('/report/file_bytes');
  const dropped = useNumber('/report/dropped_events');
  const columnsCount = useNumber('/installation/arrangement/columns');
  const rows = useNumber('/installation/arrangement/rows');
  const width = useNumber('/installation/arrangement/column_width');

  const totals = (diag?.columns ?? []).reduce(
    (acc, c) => ({
      tx: acc.tx + c.tx,
      rx: acc.rx + c.rx,
      timeouts: acc.timeouts + c.timeouts,
      errors: acc.errors + c.cobs_errors + c.msgpack_errors,
    }),
    { tx: 0, rx: 0, timeouts: 0, errors: 0 },
  );
  const worst = [...(diag?.portals ?? [])]
    .filter((p) => p.state !== 'unknown')
    .sort((a, b) => a.score - b.score)
    .slice(0, 8);
  const heatColumns = Array.from({ length: Math.max(0, columnsCount) }, () => ({
    countX: Math.max(1, width),
    countY: Math.max(1, rows),
    flipped: false,
  }));

  return (
    <div className="stack" data-av-surface="diagnostics">
      <div className="kpi-row-local">
        <Tile icon={Send} label="Tx / s" value={txRate.toFixed(1)} />
        <Tile icon={Activity} label="Rx / s" value={rxRate.toFixed(1)} />
        <Tile icon={Clock} label="ACK timeouts" value={totals.timeouts} />
        <Tile icon={Bug} label="Decode errors" value={totals.errors} />
        <Tile icon={AlertTriangle} label="Faulty units" value={faulty} tone={faulty > 0 ? 'error' : undefined} />
      </div>

      <div data-av-surface="faults">
        <Panel title={<><Layers />Installation health</>}>
          <HealthHeatmap columns={heatColumns} />
        </Panel>
      </div>

      <Panel title={<><Cable />Connections</>}>
        <div className="table-scroll">
          <table className="diag-table">
            <thead>
              <tr>
                <th>col</th>
                <th>state</th>
                <th>endpoint</th>
                <th>tx</th>
                <th>rx</th>
                <th>t/o</th>
                <th>cobs</th>
                <th>msgpack</th>
                <th>p50</th>
                <th>p90</th>
                <th>p99</th>
              </tr>
            </thead>
            <tbody>
              {(diag?.columns ?? []).map((c) => (
                <tr key={c.col} onClick={() => selection.selectColumn(c.col)}>
                  <td>{c.col + 1}</td>
                  <td>
                    <Badge tone={stateTone(c.state)}>{c.state}</Badge>
                  </td>
                  <td>{c.endpoint}</td>
                  <td>{c.tx}</td>
                  <td>{c.rx}</td>
                  <td>{c.timeouts}</td>
                  <td>{c.cobs_errors}</td>
                  <td>{c.msgpack_errors}</td>
                  <td>{c.latency_p50_ms.toFixed(0)}</td>
                  <td>{c.latency_p90_ms.toFixed(0)}</td>
                  <td>{c.latency_p99_ms.toFixed(0)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </Panel>

      <Panel title={<><AlertTriangle />Worst units</>}>
        {worst.length === 0 && <p className="placeholder">No scored portals yet.</p>}
        {worst.map((p) => (
          <button
            key={`${p.col}-${p.portal}`}
            type="button"
            className="worst-row"
            onClick={() => selection.selectPortal(p.col, p.portal)}
          >
            <span>
              col {p.col + 1} · #{p.portal}
            </span>
            <Badge tone={stateTone(p.state)}>{p.state}</Badge>
            <span className="score-bar">
              <span className="score-fill" style={{ width: `${p.score}%` }} />
            </span>
            <span>{Math.round(p.ack_rate * 100)}% ack</span>
            <span>{p.error_logs} err</span>
          </button>
        ))}
      </Panel>

      <Panel title={<><ScrollText />Fault feed</>}>
        <div className="fault-feed">
          {(diag?.recent_faults ?? [])
            .slice(-30)
            .reverse()
            .map((fault, i) => {
              // The kind is drawn as well as named: a feed is scanned for *which* kind is
              // repeating, and five kinds are distinguishable as glyphs faster than as
              // snake_case words of similar length.
              const Glyph = iconForFault(fault.kind);
              return (
              <div key={`${fault.ts_ms}-${i}`} className="fault-line">
                <span className="fault-kind">
                  <Glyph />
                  {fault.kind}
                </span>
                <span>
                  col {fault.col + 1}
                  {fault.portal != null ? ` · #${fault.portal}` : ''}
                </span>
                <span className="fault-detail">{fault.detail}</span>
                {fault.repeat > 1 && <span className="log-count">×{fault.repeat}</span>}
              </div>
              );
            })}
          {(diag?.recent_faults ?? []).length === 0 && (
            <p className="placeholder">No faults recorded this session.</p>
          )}
        </div>
      </Panel>

      <Panel title={<><FileText />Session report</>}>
        <Fact label="File" value={sessionFile.split(/[\\/]/).pop() ?? '—'} />
        <Fact label="Size" value={formatBytes(fileBytes)} />
        <Row label="Verbose packet log">
          <Toggle path="/report/verbose" />
        </Row>
        <Row label="Marker">
          <TextField path="/report/marker_text" />
        </Row>
        <div className="row wrap">
          <Action path="/report/actions/mark">Write marker</Action>
          <Action path="/report/actions/write_summary">Write summary now</Action>
        </div>
        <Fact label="Dropped events" value={dropped} />
      </Panel>
    </div>
  );
}
