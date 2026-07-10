import { fmtTs, fmtDur, fmtNum, pct, stateBadge } from './format.js';
import { drawChart } from './chart.js';
import { drawHeatmap } from './heatmap.js';

const id = new URLSearchParams(location.search).get('id');
document.getElementById('sessionId').textContent = id;
document.getElementById('exportLink').href = `/api/sessions/${id}/export`;

// ---- tabs ----
const tabs = {
  dashboard: [document.getElementById('tabDashboard'), document.getElementById('dashboard')],
  explorer: [document.getElementById('tabExplorer'), document.getElementById('explorer')],
};
function showTab(name) {
  for (const [key, [button, section]] of Object.entries(tabs)) {
    button.classList.toggle('active', key === name);
    section.style.display = key === name ? '' : 'none';
  }
}
tabs.dashboard[0].onclick = () => showTab('dashboard');
tabs.explorer[0].onclick = () => showTab('explorer');

// ---- dashboard ----
const summary = await (await fetch(`/api/sessions/${id}/summary`)).json();

document.getElementById('sessionMeta').textContent =
  `${fmtTs(summary.session.start_ts)} · ${fmtDur(summary.session.duration_ms)} · ` +
  `${summary.session.clean_exit ? 'clean exit' : 'CRASHED'}` +
  (summary.regenerated ? ' · summary regenerated from NDJSON' : '');

const totals = summary.totals ?? {};
const tiles = [
  ['Tx', totals.tx],
  ['Rx', totals.rx],
  ['ACK timeouts', totals.timeouts, totals.timeouts > 0 ? 'warn' : ''],
  ['COBS errors', totals.cobs_errors, totals.cobs_errors > 0 ? 'warn' : ''],
  ['Msgpack errors', totals.msgpack_errors, totals.msgpack_errors > 0 ? 'warn' : ''],
  ['FW error logs', totals.portal_error_logs, totals.portal_error_logs > 0 ? 'warn' : ''],
  ['Reconnects', totals.reconnects],
  ['Dropped events', totals.dropped_events, totals.dropped_events > 0 ? 'error' : ''],
];
document.getElementById('tiles').innerHTML = tiles
  .map(([label, value, cls]) => `
    <div class="tile ${cls ?? ''}">
      <div class="value">${fmtNum(value ?? 0)}</div>
      <div class="label">${label}</div>
    </div>`)
  .join('');

// heatmap
const heatCanvas = document.getElementById('heatmap');
const heatMetric = document.getElementById('heatMetric');
function renderHeatmap() {
  drawHeatmap(heatCanvas, summary.portals ?? [], heatMetric.value, (portal) => {
    document.getElementById('fCol').value = portal.col;
    document.getElementById('fPortal').value = portal.portal;
    showTab('explorer');
    resetEvents();
    loadEvents();
  });
}
heatMetric.onchange = renderHeatmap;
renderHeatmap();

// timeline chart
const timeline = await (await fetch(`/api/sessions/${id}/timeline?bucket_ms=30000`)).json();
const chartCanvas = document.getElementById('timelineChart');
function renderChart() {
  drawChart(chartCanvas, {
    t: timeline.t,
    series: [
      { label: 'tx', color: '#5a9ff2', values: timeline.tx },
      { label: 'rx', color: '#4cc966', values: timeline.rx },
      { label: 'faults', color: '#e64545', values: timeline.faults },
    ],
    markers: timeline.markers,
  });
}
renderChart();
window.addEventListener('resize', renderChart);

// columns table
document.querySelector('#columnsTable tbody').innerHTML = (summary.columns ?? [])
  .map((c) => `
    <tr>
      <td>${c.col}</td>
      <td class="mono">${c.transport ?? ''} ${c.endpoint ?? ''}</td>
      <td>${c.connects}${c.disconnects ? ` <span class="muted">(-${c.disconnects})</span>` : ''}</td>
      <td>${c.stalls || ''}</td>
      <td>${fmtNum(c.tx)}</td>
      <td>${fmtNum(c.rx)}</td>
      <td>${c.timeouts > 0 ? `<span class="badge warn">${fmtNum(c.timeouts)}</span>` : 0}</td>
      <td>${fmtNum(c.cobs_errors)}</td>
      <td>${fmtNum(c.msgpack_errors)}</td>
      <td class="mono">${(c.latency_ms?.p50 ?? 0).toFixed(0)} / ${(c.latency_ms?.p90 ?? 0).toFixed(0)} / ${(c.latency_ms?.p99 ?? 0).toFixed(0)} ms</td>
    </tr>`)
  .join('');

// top offenders
const offenderLists = [
  ['Worst ACK rate', 'worst_ack', (p) => pct(p.ack_pct)],
  ['Most timeouts', 'most_timeouts', (p) => fmtNum(p.timeouts)],
  ['Noisiest loggers', 'noisiest_loggers', (p) => `${fmtNum(p.error_logs)} err`],
  ['Rebooters', 'rebooters', (p) => `${fmtNum(p.reboots)} reboots`],
];
document.getElementById('offenders').innerHTML = offenderLists
  .map(([title, key, metric]) => {
    const rows = (summary.top_offenders?.[key] ?? [])
      .filter((p) => key === 'worst_ack' ? p.ack_pct < 100 : metric(p) !== '0' && !metric(p).startsWith('0 '))
      .slice(0, 8)
      .map((p) => `<tr><td>c${p.col} p${p.portal}</td><td>${metric(p)}</td><td>${stateBadge(p.state)}</td></tr>`)
      .join('');
    return `<div><h3 style="font-size:13px;margin:4px 0">${title}</h3>
      <table>${rows || '<tr><td class="muted">none</td></tr>'}</table></div>`;
  })
  .join('');

// fault timeline
document.querySelector('#faultsTable tbody').innerHTML = (summary.fault_timeline ?? [])
  .slice(-200)
  .reverse()
  .map((f) => `
    <tr>
      <td class="mono">${fmtTs(f.ts)}</td>
      <td class="mono">${f.ts_end !== f.ts ? fmtTs(f.ts_end) : ''}</td>
      <td><span class="badge error">${f.type}</span></td>
      <td>${f.col ?? ''}</td>
      <td>${f.portal ?? ''}</td>
      <td>${fmtNum(f.count)}</td>
      <td class="mono">${f.sample ?? ''}</td>
    </tr>`)
  .join('') || '<tr><td colspan="7" class="muted">no faults recorded 🎉</td></tr>';

// ---- explorer ----
const eventList = document.getElementById('eventList');
const fStatus = document.getElementById('fStatus');
let offset = 0;
const PAGE = 500;

function filterQuery() {
  const params = new URLSearchParams();
  const type = document.getElementById('fType').value.trim();
  const col = document.getElementById('fCol').value.trim();
  const portal = document.getElementById('fPortal').value.trim();
  const level = document.getElementById('fLevel').value;
  if (type) params.set('type', type);
  if (col) params.set('col', col);
  if (portal) params.set('portal', portal);
  if (level) params.set('min_level', level);
  params.set('limit', PAGE);
  params.set('offset', offset);
  return params;
}

const FAULTS = 'ack_timeout,cobs_error,msgpack_error,device_disconnect,health_transition,crc_error,ack_nack';

function resetEvents() {
  offset = 0;
  eventList.innerHTML = '';
}

async function loadEvents() {
  fStatus.textContent = 'loading...';
  const res = await fetch(`/api/sessions/${id}/events?${filterQuery()}`);
  const text = await res.text();
  const lines = text.split('\n').filter(Boolean);
  for (const line of lines) {
    let ev;
    try { ev = JSON.parse(line); } catch { continue; }
    const div = document.createElement('div');
    const isFault = FAULTS.includes(ev.type);
    div.className = `ev${isFault ? ' fault' : ''}`;
    const { v, ts, seq, type, ...rest } = ev;
    div.innerHTML =
      `<span class="ts">${fmtTs(ts)}</span> ` +
      `<span class="type">${type}</span> ` +
      `<span>${JSON.stringify(rest)}</span>`;
    eventList.appendChild(div);
  }
  offset += lines.length;
  fStatus.textContent = `${offset} events shown${lines.length === PAGE ? ' (more available)' : ''}`;
}

document.getElementById('fApply').onclick = () => { resetEvents(); loadEvents(); };
document.getElementById('fFaults').onclick = () => {
  document.getElementById('fType').value = FAULTS;
  resetEvents();
  loadEvents();
};
document.getElementById('fClear').onclick = () => {
  for (const fid of ['fType', 'fCol', 'fPortal']) document.getElementById(fid).value = '';
  document.getElementById('fLevel').value = '';
  resetEvents();
  loadEvents();
};
document.getElementById('loadMore').onclick = () => loadEvents();

loadEvents();
