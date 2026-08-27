//! The writer thread: owns the NDJSON file, all aggregation state, health
//! state machines, the fault storm guard, the live snapshot, and summary
//! checkpoints.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::events::{self, Event, LatencyStats, Totals};
use crate::health::{HealthInputs, PortalHealth, WindowBucket};
use crate::reporter::{Msg, ReportConfig, RxKind, SessionInfo, Shared};
use crate::snapshot::{
    ColumnDiag, ColumnState, DiagnosticsSnapshot, FaultLine, PortalDiag, PortalState,
};
use crate::summary;
use crate::time::{compact_utc_stamp, epoch_ms};

const LATENCY_BUCKET_EDGES_MS: [f32; 10] = [
    1.0,
    2.0,
    5.0,
    10.0,
    20.0,
    50.0,
    100.0,
    200.0,
    300.0,
    f32::INFINITY,
];
const FAULT_RING_SIZE: usize = 500;
const STORM_THRESHOLD_PER_SEC: u32 = 10;

#[derive(Default, Clone)]
pub(crate) struct LatencyHistogram {
    counts: [u64; 10],
    max_ms: f32,
    total: u64,
}

impl LatencyHistogram {
    fn record(&mut self, ms: f32) {
        let idx = LATENCY_BUCKET_EDGES_MS
            .iter()
            .position(|edge| ms <= *edge)
            .unwrap_or(9);
        self.counts[idx] += 1;
        self.total += 1;
        self.max_ms = self.max_ms.max(ms);
    }

    fn percentile(&self, p: f32) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        let rank = (p * self.total as f32).ceil() as u64;
        let mut seen = 0u64;
        let mut lower = 0.0f32;
        for (i, count) in self.counts.iter().enumerate() {
            let upper = if LATENCY_BUCKET_EDGES_MS[i].is_infinite() {
                self.max_ms.max(lower)
            } else {
                LATENCY_BUCKET_EDGES_MS[i]
            };
            if seen + count >= rank {
                // linear interpolation inside the bucket
                let into = if *count == 0 {
                    0.0
                } else {
                    (rank - seen) as f32 / *count as f32
                };
                return lower + (upper - lower) * into;
            }
            seen += count;
            lower = upper;
        }
        self.max_ms
    }

    pub(crate) fn stats(&self) -> LatencyStats {
        LatencyStats {
            p50: self.percentile(0.50),
            p90: self.percentile(0.90),
            p99: self.percentile(0.99),
            max: self.max_ms,
        }
    }
}

#[derive(Default, Clone)]
pub(crate) struct ColumnAgg {
    pub endpoint: String,
    pub transport: String,
    pub connected: bool,
    pub connects: u64,
    pub disconnects: u64,
    pub stalls: u64,
    pub tx: u64,
    pub rx: u64,
    pub bytes_tx: u64,
    pub bytes_rx: u64,
    pub acks: u64,
    pub timeouts: u64,
    pub cobs_errors: u64,
    pub msgpack_errors: u64,
    pub latency: LatencyHistogram,
    pub last_rx: Option<Instant>,
    pub outbox_peak: u32,
    // window (since last bus_stats tick)
    pub w_tx: u64,
    pub w_rx: u64,
    pub w_acks: u64,
    pub w_timeouts: u64,
    pub w_cobs: u64,
    pub w_msgpack: u64,
    pub w_latency: LatencyHistogram,
    pub w_outbox_peak: u32,
}

#[derive(Default, Clone)]
pub(crate) struct PortalAgg {
    pub sends: u64,
    pub ack_needing_sends: u64,
    pub acks: u64,
    pub rx: u64,
    pub timeouts: u64,
    pub latency: LatencyHistogram,
    pub last_seen: Option<Instant>,
    pub last_seen_ts_ms: Option<u64>,
    pub status_logs: u64,
    pub warning_logs: u64,
    pub error_logs: u64,
    pub top_logs: HashMap<(u8, String), u64>,
    pub version: Option<String>,
    pub uptime_ms: Option<u64>,
    pub reboots: u32,
    pub calibration_ok: [Option<bool>; 2],
    pub silent_episodes: u32,
    pub health: PortalHealth,
    // window
    pub w_bucket: WindowBucket,
    pub w_latency: LatencyHistogram,
}

pub(crate) struct State {
    pub config: ReportConfig,
    pub session: SessionInfo,
    pub session_start_ms: u64,
    pub ndjson_path: PathBuf,
    pub columns: HashMap<u8, ColumnAgg>,
    pub portals: HashMap<(u8, u8), PortalAgg>,
    pub faults: Vec<summary::FaultSpan>,
    pub recent_faults: Vec<FaultLine>,
    pub totals: Totals,
    pub seq: u64,
    pub bytes_written: u64,
}

pub(crate) fn run(
    config: ReportConfig,
    session: SessionInfo,
    rx: Receiver<Msg>,
    shared: Arc<Shared>,
) -> Option<PathBuf> {
    let start_ms = epoch_ms();
    let stamp = compact_utc_stamp(start_ms);
    let ndjson_path = config.dir.join(format!("session-{stamp}.ndjson"));
    let summary_path = config.dir.join(format!("session-{stamp}.summary.json"));

    let file = File::create(&ndjson_path).ok()?;
    let mut out = BufWriter::new(file);

    let mut state = State {
        config,
        session: session.clone(),
        session_start_ms: start_ms,
        ndjson_path: ndjson_path.clone(),
        columns: HashMap::new(),
        portals: HashMap::new(),
        faults: Vec::new(),
        recent_faults: Vec::new(),
        totals: Totals::default(),
        seq: 0,
        bytes_written: 0,
    };

    // storm guard: (col, portal, kind) -> (window start, seen, suppressed, last event)
    let mut storm: HashMap<(u8, u8, &'static str), (Instant, u32, u32, Event)> = HashMap::new();

    write_line(
        &mut out,
        &mut state,
        &Event::SessionStart {
            app_version: session.app_version.clone(),
            host: session.host.clone(),
            config: session.config.clone(),
            verbose: shared.verbose.load(Ordering::Relaxed),
        },
        None,
    );

    let start_instant = Instant::now();
    let mut last_flush = Instant::now();
    let mut last_stats = Instant::now();
    let mut last_snapshot = Instant::now();
    let mut last_checkpoint = Instant::now();
    let mut reported_drops = 0u64;
    let mut end_reason = "clean";

    'main: loop {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(Msg::Shutdown { reason }) => {
                end_reason = reason;
                break 'main;
            }
            Ok(msg) => handle_msg(msg, &mut out, &mut state, &mut storm),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break 'main,
        }

        let now = Instant::now();

        // flush storm-guard windows past 1 s
        flush_storms(&mut out, &mut state, &mut storm, false);

        // report channel drops
        let drops = shared.dropped.load(Ordering::Relaxed);
        if drops > reported_drops {
            let count = drops - reported_drops;
            reported_drops = drops;
            state.totals.dropped_events += count;
            write_line(&mut out, &mut state, &Event::DroppedEvents { count }, None);
        }

        if now.duration_since(last_stats) >= state.config.stats_interval {
            stats_tick(&mut out, &mut state, now.duration_since(last_stats));
            last_stats = now;
        }

        if now.duration_since(last_snapshot) >= Duration::from_secs(1) {
            publish_snapshot(&state, &shared);
            last_snapshot = now;
        }

        if now.duration_since(last_checkpoint) >= state.config.checkpoint_interval {
            let _ = summary::write(&state, &summary_path, start_instant.elapsed(), false);
            last_checkpoint = now;
        }

        if now.duration_since(last_flush) >= Duration::from_secs(1) {
            let _ = out.flush();
            last_flush = now;
        }
    }

    // drain remaining messages without blocking
    while let Ok(msg) = rx.try_recv() {
        if !matches!(msg, Msg::Shutdown { .. } | Msg::WriteSummary(_)) {
            handle_msg(msg, &mut out, &mut state, &mut storm);
        }
    }
    flush_storms(&mut out, &mut state, &mut storm, true);

    let duration = start_instant.elapsed();
    let totals = state.totals;
    write_line(
        &mut out,
        &mut state,
        &Event::SessionEnd {
            reason: end_reason.to_string(),
            duration_ms: duration.as_millis() as u64,
            totals,
        },
        None,
    );
    let _ = out.flush();
    publish_snapshot(&state, &shared);
    summary::write(&state, &summary_path, duration, end_reason == "clean").ok()?;
    Some(summary_path)
}

fn handle_msg(
    msg: Msg,
    out: &mut BufWriter<File>,
    state: &mut State,
    storm: &mut HashMap<(u8, u8, &'static str), (Instant, u32, u32, Event)>,
) {
    match msg {
        Msg::Line(payload) => {
            write_payload(out, state, payload);
            // Bench lines are low-rate and each one is evidence, so flush rather than risk
            // losing the tail of a run to a power cut or a kill.
            let _ = out.flush();
        }
        Msg::Event(event) => {
            ingest_event(state, &event);
            // storm guard for high-rate identical faults
            if let Some(key) = event.storm_key() {
                let now = Instant::now();
                let entry = storm.entry(key).or_insert((now, 0, 0, event.clone()));
                if now.duration_since(entry.0) >= Duration::from_secs(1) {
                    *entry = (now, 0, 0, event.clone());
                }
                entry.1 += 1;
                entry.3 = event.clone();
                if entry.1 > STORM_THRESHOLD_PER_SEC {
                    entry.2 += 1;
                    return; // suppressed; flushed by flush_storms
                }
            }
            write_line(out, state, &event, None);
            if event.is_fault() {
                let _ = out.flush();
            }
        }
        Msg::Tx {
            col,
            target,
            needs_ack,
            bytes,
        } => {
            let column = state.columns.entry(col).or_default();
            column.tx += 1;
            column.w_tx += 1;
            column.bytes_tx += bytes as u64;
            state.totals.tx += 1;
            if target > 0 {
                let portal = state.portals.entry((col, target as u8)).or_default();
                portal.sends += 1;
                if needs_ack {
                    portal.ack_needing_sends += 1;
                    portal.w_bucket.ack_needing_sends += 1;
                }
            }
        }
        Msg::Rx {
            col,
            source,
            kind,
            latency_ms,
            bytes,
        } => {
            let column = state.columns.entry(col).or_default();
            column.rx += 1;
            column.w_rx += 1;
            column.bytes_rx += bytes as u64;
            column.last_rx = Some(Instant::now());
            state.totals.rx += 1;
            if let Some(ms) = latency_ms {
                column.latency.record(ms);
                column.w_latency.record(ms);
            }
            if kind == RxKind::Ack || latency_ms.is_some() {
                column.acks += 1;
                column.w_acks += 1;
                state.totals.acks += 1;
            }
            if source > 0 {
                let portal = state.portals.entry((col, source as u8)).or_default();
                portal.rx += 1;
                portal.last_seen = Some(Instant::now());
                portal.last_seen_ts_ms = Some(epoch_ms());
                if let Some(ms) = latency_ms {
                    portal.latency.record(ms);
                    portal.w_latency.record(ms);
                    portal.acks += 1;
                    portal.w_bucket.acks += 1;
                }
            }
        }
        Msg::OutboxDepth { col, depth } => {
            let column = state.columns.entry(col).or_default();
            column.outbox_peak = column.outbox_peak.max(depth);
            column.w_outbox_peak = column.w_outbox_peak.max(depth);
        }
        Msg::WriteSummary(reply) => {
            let path = state.config.dir.join(format!(
                "session-{}.summary.json",
                compact_utc_stamp(state.session_start_ms)
            ));
            if summary::write(
                state,
                &path,
                Duration::from_millis(epoch_ms() - state.session_start_ms),
                false,
            )
            .is_ok()
            {
                let _ = reply.try_send(path);
            }
        }
        Msg::Shutdown { .. } => {}
    }
}

/// Update aggregates from a structured event.
fn ingest_event(state: &mut State, event: &Event) {
    match event {
        Event::DeviceConnect {
            col,
            transport,
            endpoint,
            ok,
            ..
        } => {
            let column = state.columns.entry(*col).or_default();
            column.endpoint = endpoint.clone();
            column.transport = transport.clone();
            if *ok {
                column.connected = true;
                column.connects += 1;
                if column.connects > 1 {
                    state.totals.reconnects += 1;
                }
            }
        }
        Event::DeviceDisconnect { col, reason, .. } => {
            let column = state.columns.entry(*col).or_default();
            column.connected = false;
            column.disconnects += 1;
            if reason == "stall" {
                column.stalls += 1;
            }
        }
        Event::AckTimeout { col, portal, .. } => {
            let column = state.columns.entry(*col).or_default();
            column.timeouts += 1;
            column.w_timeouts += 1;
            state.totals.timeouts += 1;
            let p = state.portals.entry((*col, *portal)).or_default();
            p.timeouts += 1;
            p.w_bucket.timeouts += 1;
        }
        Event::CobsError { col, .. } => {
            let column = state.columns.entry(*col).or_default();
            column.cobs_errors += 1;
            column.w_cobs += 1;
            state.totals.cobs_errors += 1;
        }
        Event::MsgpackError { col, .. } => {
            let column = state.columns.entry(*col).or_default();
            column.msgpack_errors += 1;
            column.w_msgpack += 1;
            state.totals.msgpack_errors += 1;
        }
        Event::PortalLog {
            col,
            portal,
            level,
            message,
            count,
            ..
        } => {
            let p = state.portals.entry((*col, *portal)).or_default();
            let n = *count as u64;
            match *level {
                events::LEVEL_ERROR => {
                    p.error_logs += n;
                    p.w_bucket.error_logs += count;
                    state.totals.portal_error_logs += n;
                }
                events::LEVEL_WARNING => p.warning_logs += n,
                _ => p.status_logs += n,
            }
            if *level >= events::LEVEL_WARNING {
                *p.top_logs.entry((*level, message.clone())).or_default() += n;
            }
        }
        Event::PortalStatus {
            col,
            portal,
            uptime_ms,
            version,
            mca,
            mcb,
        } => {
            let p = state.portals.entry((*col, *portal)).or_default();
            if let (Some(new), Some(old)) = (uptime_ms, p.uptime_ms) {
                if *new < old {
                    p.reboots += 1;
                }
            }
            if uptime_ms.is_some() {
                p.uptime_ms = *uptime_ms;
            }
            if version.is_some() {
                p.version = version.clone();
            }
            for (i, axis) in [mca, mcb].into_iter().enumerate() {
                if let Some(flags) = axis {
                    p.calibration_ok[i] = match (
                        flags.measure_cycle_ok,
                        flags.switches_ok,
                        flags.backlash_ok,
                        flags.home_ok,
                    ) {
                        (Some(a), Some(b), Some(c), Some(d)) => Some(a && b && c && d),
                        _ => p.calibration_ok[i],
                    };
                }
            }
        }
        _ => {}
    }

    // fault timeline (merged spans for the summary + live feed ring)
    if event.is_fault() {
        let ts = epoch_ms();
        summary::record_fault(state, event, ts);
        let line = fault_line(event, ts, 1);
        if let Some(line) = line {
            if state.recent_faults.len() >= FAULT_RING_SIZE {
                state.recent_faults.remove(0);
            }
            state.recent_faults.push(line);
        }
    }
}

pub(crate) fn fault_line(event: &Event, ts_ms: u64, repeat: u32) -> Option<FaultLine> {
    let (kind, col, portal, detail) = match event {
        Event::AckTimeout {
            col,
            portal,
            addr,
            waited_ms,
        } => (
            "ack_timeout",
            *col,
            Some(*portal),
            format!("addr={addr} waited={waited_ms}ms"),
        ),
        Event::AckNack { col, portal, addr } => {
            ("ack_nack", *col, Some(*portal), format!("addr={addr}"))
        }
        Event::CobsError { col, detail } => ("cobs_error", *col, None, detail.clone()),
        Event::MsgpackError { col, detail, .. } => ("msgpack_error", *col, None, detail.clone()),
        Event::CrcError { col, expected, got } => (
            "crc_error",
            *col,
            None,
            format!("expected {expected:04X} got {got:04X}"),
        ),
        Event::DeviceDisconnect { col, reason, error } => (
            "device_disconnect",
            *col,
            None,
            match error {
                Some(e) => format!("{reason}: {e}"),
                None => reason.clone(),
            },
        ),
        Event::HealthTransition {
            col,
            portal,
            from,
            to,
            reason,
            ..
        } => (
            "health_transition",
            *col,
            *portal,
            format!("{from} -> {to}: {reason}"),
        ),
        _ => return None,
    };
    Some(FaultLine {
        ts_ms,
        kind: kind.to_string(),
        col,
        portal,
        detail,
        repeat,
    })
}

fn flush_storms(
    out: &mut BufWriter<File>,
    state: &mut State,
    storm: &mut HashMap<(u8, u8, &'static str), (Instant, u32, u32, Event)>,
    force: bool,
) {
    let now = Instant::now();
    storm.retain(|_, (start, _seen, suppressed, last_event)| {
        let expired = now.duration_since(*start) >= Duration::from_secs(1);
        if (expired || force) && *suppressed > 0 {
            let repeat = *suppressed;
            let event = last_event.clone();
            write_line(out, state, &event, Some(repeat));
        }
        !(expired || force)
    });
}

fn write_line(out: &mut BufWriter<File>, state: &mut State, event: &Event, repeat: Option<u32>) {
    state.seq += 1;
    let line = events::to_line(event, epoch_ms(), state.seq, repeat);
    if let Ok(text) = serde_json::to_string(&line) {
        state.bytes_written += text.len() as u64 + 1;
        let _ = writeln!(out, "{text}");
    }
}

/// bus_stats emission + per-portal health ticks.
fn stats_tick(out: &mut BufWriter<File>, state: &mut State, window: Duration) {
    let cols: Vec<u8> = state.columns.keys().copied().collect();
    for col in cols {
        let stats_event = {
            let column = state.columns.get_mut(&col).unwrap();
            let event = Event::BusStats {
                col,
                window_ms: window.as_millis() as u64,
                tx: column.w_tx,
                rx: column.w_rx,
                acks: column.w_acks,
                timeouts: column.w_timeouts,
                cobs_errors: column.w_cobs,
                msgpack_errors: column.w_msgpack,
                latency_ms: column.w_latency.stats(),
                outbox_peak: column.w_outbox_peak,
            };
            column.w_tx = 0;
            column.w_rx = 0;
            column.w_acks = 0;
            column.w_timeouts = 0;
            column.w_cobs = 0;
            column.w_msgpack = 0;
            column.w_latency = LatencyHistogram::default();
            column.w_outbox_peak = 0;
            event
        };
        write_line(out, state, &stats_event, None);
    }

    // health ticks
    let keys: Vec<(u8, u8)> = state.portals.keys().copied().collect();
    let mut transitions = Vec::new();
    for key in keys {
        let portal = state.portals.get_mut(&key).unwrap();
        let mut bucket = portal.w_bucket;
        bucket.latency_p90_ms = portal.w_latency.stats().p90;
        portal.w_bucket = WindowBucket::default();
        portal.w_latency = LatencyHistogram::default();

        let calibration_bad = match portal.calibration_ok {
            [Some(a), Some(b)] => Some(!(a && b)),
            [Some(a), None] | [None, Some(a)] => Some(!a),
            _ => None,
        };
        let inputs = HealthInputs {
            last_seen_age_ms: portal.last_seen.map(|t| t.elapsed().as_millis() as u64),
            response_window_ms: 300.0,
            calibration_bad,
            poll_interval_ms: 10_000,
        };
        let outcome = portal.health.tick(bucket, &inputs);
        if let Some((from, to, reason)) = outcome.transition {
            if to == PortalState::Silent {
                portal.silent_episodes += 1;
            }
            transitions.push(Event::HealthTransition {
                scope: "portal".into(),
                col: key.0,
                portal: Some(key.1),
                from: from.as_str().into(),
                to: to.as_str().into(),
                reason,
                score: outcome.score,
            });
        }
    }
    for event in transitions {
        ingest_event(state, &event);
        write_line(out, state, &event, None);
    }
    let _ = out.flush();
}

fn publish_snapshot(state: &State, shared: &Arc<Shared>) {
    let mut columns: Vec<ColumnDiag> = state
        .columns
        .iter()
        .map(|(col, agg)| {
            let stats = agg.latency.stats();
            let state = if !agg.connected {
                ColumnState::Disconnected
            } else if agg.w_cobs + agg.w_msgpack > 5 {
                ColumnState::Noisy
            } else if agg.w_tx > 0
                && agg
                    .last_rx
                    .map(|t| t.elapsed() > Duration::from_secs(5))
                    .unwrap_or(true)
            {
                ColumnState::Stalled
            } else {
                ColumnState::Connected
            };
            ColumnDiag {
                col: *col,
                state,
                endpoint: agg.endpoint.clone(),
                tx: agg.tx,
                rx: agg.rx,
                acks: agg.acks,
                timeouts: agg.timeouts,
                cobs_errors: agg.cobs_errors,
                msgpack_errors: agg.msgpack_errors,
                reconnects: agg.connects.saturating_sub(1),
                latency_p50_ms: stats.p50,
                latency_p90_ms: stats.p90,
                latency_p99_ms: stats.p99,
                last_rx_age_ms: agg.last_rx.map(|t| t.elapsed().as_millis() as u64),
                outbox_peak: agg.outbox_peak,
            }
        })
        .collect();
    columns.sort_by_key(|c| c.col);

    let mut portals: Vec<PortalDiag> = state
        .portals
        .iter()
        .map(|((col, portal), agg)| PortalDiag {
            col: *col,
            portal: *portal,
            state: agg.health.state,
            score: agg.health.score,
            ack_rate: if agg.ack_needing_sends > 0 {
                agg.acks as f32 / agg.ack_needing_sends as f32
            } else {
                1.0
            },
            latency_p90_ms: agg.latency.stats().p90,
            sends: agg.sends,
            timeouts: agg.timeouts,
            last_seen_age_ms: agg.last_seen.map(|t| t.elapsed().as_millis() as u64),
            error_logs: agg.error_logs,
            warning_logs: agg.warning_logs,
            version: agg.version.clone(),
            uptime_ms: agg.uptime_ms,
            reboots: agg.reboots,
            calibration_ok: agg.calibration_ok,
        })
        .collect();
    portals.sort_by_key(|p| (p.col, p.portal));

    let snapshot = DiagnosticsSnapshot {
        session_file: state.ndjson_path.display().to_string(),
        session_start_ms: state.session_start_ms,
        file_bytes: state.bytes_written,
        dropped_events: state.totals.dropped_events,
        verbose: shared.verbose.load(Ordering::Relaxed),
        columns,
        portals,
        recent_faults: state.recent_faults.clone(),
    };
    *shared.snapshot.lock().unwrap() = Arc::new(snapshot);
}

/// Write a caller-shaped payload, stamped like every other line.
///
/// Shares `state.seq` with [`write_line`] so one session file has one monotonic sequence
/// regardless of which vocabulary produced a given line. The storm guard deliberately does not
/// apply: it keys on the bus `Event` enum, and a caller's own events are its business to pace.
fn write_payload(out: &mut BufWriter<File>, state: &mut State, mut payload: serde_json::Value) {
    state.seq += 1;
    if let Some(object) = payload.as_object_mut() {
        object.insert("v".into(), events::SCHEMA_VERSION.into());
        object.insert("ts".into(), epoch_ms().into());
        object.insert("seq".into(), state.seq.into());
    }
    if let Ok(text) = serde_json::to_string(&payload) {
        state.bytes_written += text.len() as u64 + 1;
        let _ = writeln!(out, "{text}");
    }
}
