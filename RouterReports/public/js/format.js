export const fmtTs = (ms) => (ms == null ? '—' : new Date(ms).toISOString().replace('T', ' ').slice(0, 19));
export const fmtDur = (ms) => {
  if (ms == null) return '—';
  const s = Math.floor(ms / 1000);
  if (s < 90) return `${s}s`;
  if (s < 5400) return `${Math.floor(s / 60)}m ${s % 60}s`;
  return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
};
export const fmtBytes = (b) => {
  if (b < 1024) return `${b} B`;
  if (b < 1048576) return `${(b / 1024).toFixed(1)} KB`;
  return `${(b / 1048576).toFixed(1)} MB`;
};
export const fmtNum = (n) => (n == null ? '—' : n.toLocaleString('en-US'));
export const pct = (v) => (v == null ? '—' : `${v.toFixed(1)}%`);

export const stateBadge = (state) => {
  const cls = { ok: 'ok', degraded: 'warn', faulty: 'error', silent: 'error', unknown: 'muted' }[state] ?? 'muted';
  return `<span class="badge ${cls}">${state}</span>`;
};
