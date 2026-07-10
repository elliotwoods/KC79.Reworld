// Time-bucketed aggregation for charts: server-side reduction so large
// NDJSON files never reach the browser raw. Output is columnar (uPlot's
// preferred shape).

const FAULT_TYPES = new Set([
  'ack_timeout',
  'cobs_error',
  'msgpack_error',
  'device_disconnect',
  'health_transition',
]);

/**
 * @param events async iterable of parsed events
 * @param bucketMs bucket width (>= 1000)
 * @param col optional column filter
 */
export async function bucketize(events, bucketMs, col = null) {
  const buckets = new Map(); // bucketStart -> aggregates

  const bucket = (ts) => {
    const start = Math.floor(ts / bucketMs) * bucketMs;
    if (!buckets.has(start)) {
      buckets.set(start, {
        tx: 0,
        rx: 0,
        timeouts: 0,
        decode_errors: 0,
        faults: 0,
        latency_p90: 0,
        markers: [],
      });
    }
    return buckets.get(start);
  };

  for await (const ev of events) {
    if (typeof ev.ts !== 'number') continue;
    if (col != null && ev.col != null && ev.col !== col) continue;
    const b = bucket(ev.ts);
    const repeat = ev.repeat ?? 1;
    switch (ev.type) {
      case 'bus_stats':
        b.tx += ev.tx ?? 0;
        b.rx += ev.rx ?? 0;
        b.latency_p90 = Math.max(b.latency_p90, ev.latency_ms?.p90 ?? 0);
        break;
      case 'ack_timeout':
        b.timeouts += repeat;
        b.faults += repeat;
        break;
      case 'cobs_error':
      case 'msgpack_error':
        b.decode_errors += repeat;
        b.faults += repeat;
        break;
      case 'marker':
        b.markers.push(ev.label);
        break;
      default:
        if (FAULT_TYPES.has(ev.type)) b.faults += repeat;
    }
  }

  const times = [...buckets.keys()].sort((a, b) => a - b);
  const series = (key) => times.map((t) => buckets.get(t)[key]);
  return {
    bucket_ms: bucketMs,
    // uPlot expects seconds
    t: times.map((t) => t / 1000),
    tx: series('tx'),
    rx: series('rx'),
    timeouts: series('timeouts'),
    decode_errors: series('decode_errors'),
    faults: series('faults'),
    latency_p90: series('latency_p90'),
    markers: times
      .map((t) => ({ t: t / 1000, labels: buckets.get(t).markers }))
      .filter((m) => m.labels.length > 0),
  };
}
