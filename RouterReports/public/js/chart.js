// Minimal multi-series time line chart on canvas (no dependencies).
// Data: { t: [seconds...], series: [{ label, color, values }], markers? }

export function drawChart(canvas, data) {
  const dpr = window.devicePixelRatio || 1;
  const cssWidth = canvas.clientWidth || 800;
  const cssHeight = canvas.clientHeight || 220;
  canvas.width = cssWidth * dpr;
  canvas.height = cssHeight * dpr;
  const ctx = canvas.getContext('2d');
  ctx.scale(dpr, dpr);
  ctx.clearRect(0, 0, cssWidth, cssHeight);

  const pad = { l: 46, r: 10, t: 8, b: 20 };
  const w = cssWidth - pad.l - pad.r;
  const h = cssHeight - pad.t - pad.b;
  const { t } = data;
  if (!t || t.length === 0) {
    ctx.fillStyle = '#8a919c';
    ctx.fillText('no data', pad.l, pad.t + 20);
    return;
  }

  const t0 = t[0];
  const t1 = t[t.length - 1] || t0 + 1;
  const maxValue = Math.max(1, ...data.series.flatMap((s) => s.values));
  const x = (ts) => pad.l + ((ts - t0) / Math.max(t1 - t0, 1)) * w;
  const y = (v) => pad.t + h - (v / maxValue) * h;

  // grid + y labels
  ctx.strokeStyle = '#2a2e36';
  ctx.fillStyle = '#8a919c';
  ctx.font = '10px Consolas, monospace';
  ctx.lineWidth = 1;
  for (let i = 0; i <= 4; i++) {
    const value = (maxValue / 4) * i;
    const yy = y(value);
    ctx.beginPath();
    ctx.moveTo(pad.l, yy);
    ctx.lineTo(pad.l + w, yy);
    ctx.stroke();
    ctx.fillText(value >= 100 ? Math.round(value).toString() : value.toFixed(1), 4, yy + 3);
  }
  // x labels (start / mid / end)
  const timeLabel = (ts) => new Date(ts * 1000).toISOString().slice(11, 19);
  ctx.fillText(timeLabel(t0), pad.l, cssHeight - 6);
  ctx.fillText(timeLabel((t0 + t1) / 2), pad.l + w / 2 - 24, cssHeight - 6);
  ctx.fillText(timeLabel(t1), pad.l + w - 48, cssHeight - 6);

  // markers
  for (const marker of data.markers ?? []) {
    const xx = x(marker.t);
    ctx.strokeStyle = '#f2bf33';
    ctx.setLineDash([3, 3]);
    ctx.beginPath();
    ctx.moveTo(xx, pad.t);
    ctx.lineTo(xx, pad.t + h);
    ctx.stroke();
    ctx.setLineDash([]);
  }

  // series
  for (const series of data.series) {
    ctx.strokeStyle = series.color;
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    series.values.forEach((v, i) => {
      const xx = x(t[i]);
      const yy = y(v);
      if (i === 0) ctx.moveTo(xx, yy);
      else ctx.lineTo(xx, yy);
    });
    ctx.stroke();
  }
}
