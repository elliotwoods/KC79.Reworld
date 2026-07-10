//! Session summary JSON: totals, per-column/per-portal tables, deduped fault
//! timeline, top offenders. Written at shutdown, on demand, and checkpointed
//! periodically (atomic temp+rename). The Node.js viewer can regenerate an
//! equivalent summary from the NDJSON via `lib/reduce.js`.

use std::io;
use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value as Json};

use crate::events::Event;
use crate::time::epoch_ms;
use crate::writer::State;

/// A merged fault-timeline entry (consecutive same-key faults within 30 s
/// merge into one span).
#[derive(Debug, Clone)]
pub struct FaultSpan {
    pub ts: u64,
    pub ts_end: u64,
    pub kind: String,
    pub col: u8,
    pub portal: Option<u8>,
    pub count: u64,
    pub sample: String,
}

const MERGE_WINDOW_MS: u64 = 30_000;
const MAX_FAULT_SPANS: usize = 5_000;

pub(crate) fn record_fault(state: &mut State, event: &Event, ts: u64) {
    let Some(line) = crate::writer::fault_line(event, ts, 1) else {
        return;
    };
    if let Some(last) = state.faults.last_mut() {
        if last.kind == line.kind
            && last.col == line.col
            && last.portal == line.portal
            && ts.saturating_sub(last.ts_end) <= MERGE_WINDOW_MS
        {
            last.count += 1;
            last.ts_end = ts;
            return;
        }
    }
    if state.faults.len() >= MAX_FAULT_SPANS {
        return; // capped; totals still count everything
    }
    state.faults.push(FaultSpan {
        ts,
        ts_end: ts,
        kind: line.kind,
        col: line.col,
        portal: line.portal,
        count: 1,
        sample: line.detail,
    });
}

pub(crate) fn write(
    state: &State,
    path: &Path,
    duration: Duration,
    clean_exit: bool,
) -> io::Result<()> {
    let doc = build(state, duration, clean_exit);
    let text = serde_json::to_string_pretty(&doc)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    std::fs::rename(&tmp, path)
}

pub(crate) fn build(state: &State, duration: Duration, clean_exit: bool) -> Json {
    let mut columns: Vec<Json> = state
        .columns
        .iter()
        .map(|(col, agg)| {
            let stats = agg.latency.stats();
            json!({
                "col": col,
                "endpoint": agg.endpoint,
                "transport": agg.transport,
                "connects": agg.connects,
                "disconnects": agg.disconnects,
                "stalls": agg.stalls,
                "tx": agg.tx,
                "rx": agg.rx,
                "bytes_tx": agg.bytes_tx,
                "bytes_rx": agg.bytes_rx,
                "acks": agg.acks,
                "timeouts": agg.timeouts,
                "cobs_errors": agg.cobs_errors,
                "msgpack_errors": agg.msgpack_errors,
                "latency_ms": { "p50": stats.p50, "p90": stats.p90, "p99": stats.p99, "max": stats.max },
                "outbox_peak": agg.outbox_peak,
            })
        })
        .collect();
    columns.sort_by_key(|c| c["col"].as_u64());

    let mut portals: Vec<Json> = state
        .portals
        .iter()
        .map(|((col, portal), agg)| {
            let mut top_logs: Vec<(&(u8, String), &u64)> = agg.top_logs.iter().collect();
            top_logs.sort_by(|a, b| b.1.cmp(a.1));
            let top_logs: Vec<Json> = top_logs
                .into_iter()
                .take(5)
                .map(|((level, message), count)| json!({ "level": level, "message": message, "count": count }))
                .collect();
            let ack_pct = if agg.ack_needing_sends > 0 {
                100.0 * agg.acks as f64 / agg.ack_needing_sends as f64
            } else {
                100.0
            };
            let stats = agg.latency.stats();
            json!({
                "col": col,
                "portal": portal,
                "sends": agg.sends,
                "rx": agg.rx,
                "ack_pct": ack_pct,
                "timeouts": agg.timeouts,
                "latency_ms": { "p50": stats.p50, "p90": stats.p90, "max": stats.max },
                "last_seen_ts": agg.last_seen_ts_ms,
                "silent_episodes": agg.silent_episodes,
                "log_counts": { "status": agg.status_logs, "warning": agg.warning_logs, "error": agg.error_logs },
                "top_logs": top_logs,
                "version": agg.version,
                "reboots": agg.reboots,
                "calibration_ok": agg.calibration_ok,
                "final_state": agg.health.state.as_str(),
                "final_score": agg.health.score,
            })
        })
        .collect();
    portals.sort_by_key(|p| (p["col"].as_u64(), p["portal"].as_u64()));

    let fault_timeline: Vec<Json> = state
        .faults
        .iter()
        .map(|span| {
            json!({
                "ts": span.ts,
                "ts_end": span.ts_end,
                "type": span.kind,
                "col": span.col,
                "portal": span.portal,
                "count": span.count,
                "sample": span.sample,
            })
        })
        .collect();

    let top = |metric: fn(&Json) -> f64, ascending: bool, count: usize| -> Vec<Json> {
        let mut sorted = portals.clone();
        sorted.sort_by(|a, b| {
            let (ma, mb) = (metric(a), metric(b));
            if ascending {
                ma.partial_cmp(&mb).unwrap_or(std::cmp::Ordering::Equal)
            } else {
                mb.partial_cmp(&ma).unwrap_or(std::cmp::Ordering::Equal)
            }
        });
        sorted
            .into_iter()
            .take(count)
            .map(|p| {
                json!({
                    "col": p["col"], "portal": p["portal"],
                    "ack_pct": p["ack_pct"], "timeouts": p["timeouts"],
                    "error_logs": p["log_counts"]["error"], "reboots": p["reboots"],
                    "state": p["final_state"],
                })
            })
            .collect()
    };

    json!({
        "v": 1,
        "session": {
            "start_ts": state.session_start_ms,
            "end_ts": epoch_ms(),
            "duration_ms": duration.as_millis() as u64,
            "clean_exit": clean_exit,
            "app_version": state.session.app_version,
            "host": state.session.host,
            "ndjson_files": [state.ndjson_path.file_name().map(|f| f.to_string_lossy().to_string())],
            "config": state.session.config,
            "dropped_events": state.totals.dropped_events,
        },
        "totals": state.totals,
        "columns": columns,
        "portals": portals,
        "fault_timeline": fault_timeline,
        "top_offenders": {
            "worst_ack": top(|p| p["ack_pct"].as_f64().unwrap_or(100.0), true, 10),
            "most_timeouts": top(|p| p["timeouts"].as_f64().unwrap_or(0.0), false, 10),
            "noisiest_loggers": top(|p| p["log_counts"]["error"].as_f64().unwrap_or(0.0), false, 10),
            "rebooters": top(|p| p["reboots"].as_f64().unwrap_or(0.0), false, 10),
        },
    })
}
