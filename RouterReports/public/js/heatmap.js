// Installation-shaped portal heatmap: one cell per portal, grouped by
// column, colored by the selected metric. Hover = tooltip, click = callback.

const STATE_COLORS = {
  ok: '#2e9e4c',
  degraded: '#c9992a',
  faulty: '#c23535',
  silent: '#7a1fa0',
  unknown: '#3a3f48',
};

function metricColor(portal, metric) {
  if (metric === 'state') return STATE_COLORS[portal.final_state] ?? STATE_COLORS.unknown;
  let ratio;
  if (metric === 'ack') {
    ratio = 1 - (portal.ack_pct ?? 100) / 100; // 0 good -> 1 bad
  } else if (metric === 'timeouts') {
    ratio = Math.min((portal.timeouts ?? 0) / 50, 1);
  } else {
    ratio = Math.min((portal.log_counts?.error ?? 0) / 50, 1);
  }
  const g = Math.round(158 - 120 * ratio);
  const r = Math.round(46 + 160 * ratio);
  return `rgb(${r}, ${g}, 60)`;
}

export function drawHeatmap(canvas, portals, metric, onClick) {
  const byCol = new Map();
  for (const portal of portals) {
    if (!byCol.has(portal.col)) byCol.set(portal.col, []);
    byCol.get(portal.col).push(portal);
  }
  const cols = [...byCol.keys()].sort((a, b) => a - b);
  const maxPerColumn = Math.max(1, ...[...byCol.values()].map((v) => v.length));

  const cell = 16;
  const gap = 2;
  const colGap = 8;
  const width = cols.length * (cell + colGap) + colGap;
  const height = maxPerColumn * (cell + gap) + 24;
  canvas.width = width;
  canvas.height = height;
  canvas.style.width = `${width}px`;
  canvas.style.height = `${height}px`;

  const ctx = canvas.getContext('2d');
  ctx.clearRect(0, 0, width, height);
  ctx.font = '9px Consolas, monospace';

  const hits = [];
  cols.forEach((col, ci) => {
    const x = colGap + ci * (cell + colGap);
    ctx.fillStyle = '#8a919c';
    ctx.fillText(String(col), x + 2, height - 8);
    const list = byCol.get(col).sort((a, b) => a.portal - b.portal);
    list.forEach((portal, pi) => {
      // portal 1 at the bottom (matches the physical installation)
      const y = (maxPerColumn - 1 - pi) * (cell + gap) + 2;
      ctx.fillStyle = metricColor(portal, metric);
      ctx.fillRect(x, y, cell, cell);
      hits.push({ x, y, w: cell, h: cell, portal });
    });
  });

  canvas.onmousemove = (event) => {
    const rect = canvas.getBoundingClientRect();
    const mx = event.clientX - rect.left;
    const my = event.clientY - rect.top;
    const hit = hits.find((h) => mx >= h.x && mx < h.x + h.w && my >= h.y && my < h.y + h.h);
    canvas.title = hit
      ? `col ${hit.portal.col} portal ${hit.portal.portal}\n` +
        `state ${hit.portal.final_state} (score ${hit.portal.final_score})\n` +
        `ack ${hit.portal.ack_pct?.toFixed(1)}%  timeouts ${hit.portal.timeouts}\n` +
        `error logs ${hit.portal.log_counts?.error ?? 0}  fw ${hit.portal.version ?? '?'}`
      : '';
    canvas.style.cursor = hit ? 'pointer' : 'default';
  };
  canvas.onclick = (event) => {
    const rect = canvas.getBoundingClientRect();
    const mx = event.clientX - rect.left;
    const my = event.clientY - rect.top;
    const hit = hits.find((h) => mx >= h.x && mx < h.x + h.w && my >= h.y && my < h.y + h.h);
    if (hit && onClick) onClick(hit.portal);
  };
}
