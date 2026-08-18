//! The one thread that owns the hardware.
//!
//! It holds the [`Bench`], ticks it, and mirrors the result into bus parameters and into the
//! shared snapshot the HTTP routes read. Nothing else in the process touches a link — see the
//! transport module docs in `bench-core` for why that is a correctness property.
//!
//! # Two front doors, one queue
//!
//! Requests arrive from the page as **counter bumps** on `/actions/*`, and from an agent as
//! HTTP posts that push onto [`Shared::requests`]. Both are drained here, in one place, in
//! arrival order. That is what makes "a human watching a plot while an agent drives" a
//! supported mode rather than a race.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use av_gui_bus::{Bus, Value};
use bench_core::bench::{Bench, Origin, Outcome, RunStatus};
use bench_core::dut::{Axis, FirmwareKind, GearRatio};
use bench_core::engine::Phase;
use bench_core::plan::Plan;
use bench_core::state::BenchState;
use bench_core::transport::{LinkKind, Op};

use crate::schema::{self, AxisParams, Params};

/// Worker tick period.
///
/// 20 ms is fast enough that a 60 Hz status stream from the bench firmware is read without the
/// serial buffer growing, and slow enough that an idle bench costs nothing.
const TICK: Duration = Duration::from_millis(20);

/// Something an agent asked for over HTTP.
#[derive(Debug, Clone)]
pub enum Request {
    Connect { kind: LinkKind, endpoint: String },
    Disconnect,
    Submit(Op),
    Run { plan: Box<Plan>, origin: Origin },
    Abort,
}

/// What the HTTP routes read and write. The worker owns the hardware; this is the mirror.
#[derive(Default)]
pub struct Shared {
    pub state: Mutex<BenchState>,
    pub run: Mutex<Option<RunStatus>>,
    pub last: Mutex<Option<Outcome>>,
    /// Log lines, already rendered, with their cursor.
    pub log: Mutex<Vec<serde_json::Value>>,
    /// Requests waiting to be drained by the worker.
    pub requests: Mutex<Vec<Request>>,
    /// Filled in by the worker when a `Run` request could not start.
    pub last_start_error: Mutex<Option<String>>,
}

impl Shared {
    pub fn push(&self, request: Request) {
        self.requests.lock().unwrap().push(request);
    }
}

pub struct Worker {
    bus: Arc<Bus>,
    params: Params,
    bench: Bench,
    shared: Arc<Shared>,
    started: Instant,
    /// Last seen value of each action counter, so only a *change* fires.
    action_seen: Vec<i64>,
    plans_dir: std::path::PathBuf,
    cue_seq: i64,
    last_seq: i64,
    log_cursor: u64,
    /// Last heartbeat value seen from the page, and when we saw it change.
    heartbeat: (i64, u64),
}

/// How long a page may go quiet before it counts as gone.
///
/// The page beats once a second, so five seconds is several missed beats rather than one
/// unlucky scheduling gap.
const HEARTBEAT_STALE_MS: u64 = 5_000;

impl Worker {
    pub fn new(
        bus: Arc<Bus>,
        params: Params,
        bench: Bench,
        shared: Arc<Shared>,
        plans_dir: std::path::PathBuf,
    ) -> Self {
        let action_count = params.actions.len();
        Self {
            bus,
            params,
            bench,
            shared,
            started: Instant::now(),
            // Adopt whatever the counters currently read rather than zero: a worker that
            // restarts must not replay every action the page has ever taken.
            action_seen: vec![i64::MIN; action_count],
            plans_dir,
            cue_seq: 0,
            last_seq: 0,
            log_cursor: 0,
            heartbeat: (0, 0),
        }
    }

    /// Whether the page has gone quiet.
    ///
    /// **This is deliberately narrower than PortalFlasher's dead-man.** There, a stale heartbeat
    /// disarms the rig, because an unattended flasher with a pogo fixture is dangerous. Here it
    /// only blocks *starting* destructive work from the page; it never cancels a run in flight,
    /// because closing a browser tab must not abort an eight-hour soak. And it never gates the
    /// HTTP door at all: an agent driving `ptb` has no page and is not supposed to need one.
    fn page_is_stale(&mut self, now: u64) -> bool {
        let value = schema::get_i64(&self.bus, self.params.ui_heartbeat);
        if value != self.heartbeat.0 {
            self.heartbeat = (value, now);
        }
        // Nothing has ever beaten: the page has not connected yet, which is not the same as one
        // that connected and stopped. Treat it as stale either way -- both mean "nobody is
        // watching" -- but only after the same grace period, so a fresh start is not blocked.
        now.saturating_sub(self.heartbeat.1) > HEARTBEAT_STALE_MS
    }

    fn now_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    pub fn run(mut self) {
        // First pass only reads the counters, so nothing fires on startup.
        for (index, (_, id)) in self.params.actions.iter().enumerate() {
            self.action_seen[index] = schema::get_i64(&self.bus, *id);
        }

        loop {
            let now = self.now_ms();
            self.handle_actions(now);
            self.handle_requests(now);

            if let Some(outcome) = self.bench.tick(now) {
                self.publish_outcome(&outcome);
                *self.shared.last.lock().unwrap() = Some(outcome);
            }

            self.publish(now);
            std::thread::sleep(TICK);
        }
    }

    /// Turn counter bumps from the page into work.
    fn handle_actions(&mut self, now: u64) {
        for index in 0..self.params.actions.len() {
            let (name, id) = self.params.actions[index];
            let value = schema::get_i64(&self.bus, id);
            if value == self.action_seen[index] {
                continue;
            }
            self.action_seen[index] = value;
            self.act(name, now);
        }
    }

    fn act(&mut self, name: &str, now: u64) {
        match name {
            "connect" => {
                let kind = transport_from(schema::get_u32(&self.bus, self.params.transport_desired));
                let endpoint = schema::get_text(&self.bus, self.params.transport_port);
                match kind {
                    Some(kind) => {
                        if let Err(error) = self.bench.connect(kind, &endpoint, now) {
                            let _ = self.bus.set_text(self.params.transport_detail, &error);
                        }
                        self.cue(if self.bench.state().link.connected { "connected" } else { "lost" });
                    }
                    None => self.bench.note(now, bench_core::LOG_LEVEL_WARNING, "pick a transport first"),
                }
            }
            "disconnect" => {
                self.bench.disconnect(now);
                self.cue("lost");
            }
            "identify" => self.bench.submit(Op::Identify),
            "escape" => self.bench.submit(Op::Escape),
            "home_a" => self.bench.submit(Op::Home { axis: Axis::A }),
            "home_b" => self.bench.submit(Op::Home { axis: Axis::B }),
            "unjam_a" => self.bench.submit(Op::Unjam { axis: Axis::A }),
            "unjam_b" => self.bench.submit(Op::Unjam { axis: Axis::B }),
            "abort" => {
                if self.bench.abort(now) {
                    self.cue("abort");
                }
            }
            "marker" => self.bench.note(now, bench_core::LOG_LEVEL_STATUS, "operator marker"),
            "run" => {
                if self.page_is_stale(now) {
                    // The press arrived, so a page exists -- but it has not beaten in five
                    // seconds, which means the bus is not carrying its writes. Starting a
                    // minutes-long routine that nobody can then see or abort is worse than
                    // refusing it.
                    self.bench.note(
                        now,
                        bench_core::LOG_LEVEL_WARNING,
                        "refused: this page has lost contact with the bench",
                    );
                    self.cue("attention");
                    return;
                }
                let name = schema::get_text(&self.bus, self.params.plan_selected);
                let name = if name.is_empty() { "startup".to_string() } else { name };
                self.start_named(&name, Origin::Gui, now);
            }
            // Rescan and flash land with M4; saying so beats a button that silently does
            // nothing when pressed.
            other => self.bench.note(
                now,
                bench_core::LOG_LEVEL_WARNING,
                format!("`{other}` is not wired up yet"),
            ),
        }
    }

    fn start_named(&mut self, name: &str, origin: Origin, now: u64) {
        let path = self.plans_dir.join(format!("{name}.toml"));
        match bench_core::plan::load(&path) {
            Ok(plan) => self.start(plan, origin, now),
            Err(error) => {
                *self.shared.last_start_error.lock().unwrap() = Some(error.clone());
                self.bench.note(now, bench_core::LOG_LEVEL_ERROR, error);
            }
        }
    }

    fn start(&mut self, plan: Plan, origin: Origin, now: u64) {
        match self.bench.start(plan, origin, now) {
            Ok(_) => {
                *self.shared.last_start_error.lock().unwrap() = None;
                self.cue("run-start");
            }
            Err(error) => {
                let detail = error.to_string();
                *self.shared.last_start_error.lock().unwrap() = Some(detail.clone());
                // A refused plan is a result the operator needs to see, not a silent no-op.
                self.bench.note(now, bench_core::LOG_LEVEL_ERROR, detail);
                self.cue("attention");
            }
        }
    }

    fn handle_requests(&mut self, now: u64) {
        let requests: Vec<Request> = std::mem::take(&mut *self.shared.requests.lock().unwrap());
        for request in requests {
            match request {
                Request::Connect { kind, endpoint } => {
                    let _ = self.bench.connect(kind, &endpoint, now);
                    // Reflect an agent-initiated connect back into the page controls. Without
                    // this the Link selector still reads "none" while the bench is plainly
                    // connected -- the page would be describing an intention nobody holds.
                    let _ = self.bus.set(
                        self.params.transport_desired,
                        Value::Enum(transport_value(kind)),
                    );
                    let _ = self.bus.set_text(self.params.transport_port, &endpoint);
                }
                Request::Disconnect => self.bench.disconnect(now),
                Request::Submit(op) => self.bench.submit(op),
                Request::Run { plan, origin } => self.start(*plan, origin, now),
                Request::Abort => {
                    self.bench.abort(now);
                }
            }
        }
    }

    fn cue(&mut self, name: &str) {
        let value = schema::CUES.iter().find(|(_, n)| *n == name).map(|(v, _)| *v).unwrap_or(0);
        self.cue_seq += 1;
        let _ = self.bus.set(self.params.cue, Value::Enum(value));
        let _ = self.bus.set(self.params.cue_seq, Value::I64(self.cue_seq));
    }

    /// Mirror everything the bench knows into the bus and the shared snapshot.
    fn publish(&mut self, now: u64) {
        let state = self.bench.state().clone();
        let set = |id, value| {
            let _ = self.bus.set(id, value);
        };
        let text = |id, value: &str| {
            let _ = self.bus.set_text(id, value);
        };

        set(self.params.transport_connected, Value::Bool(state.link.connected));
        set(
            self.params.transport_observed,
            Value::Enum(state.link.kind.map(transport_value).unwrap_or(0)),
        );
        text(self.params.transport_detail, state.link.detail.as_deref().unwrap_or(""));

        set(self.params.dut_present, Value::Bool(state.dut.present));
        set(self.params.dut_firmware_kind, Value::Enum(firmware_value(state.dut.firmware)));
        text(self.params.dut_version, state.dut.version.as_deref().unwrap_or(""));
        set(self.params.dut_uptime_s, Value::I32(state.dut.uptime_s.unwrap_or(0)));
        set(self.params.dut_ratio, Value::Enum(ratio_value(state.dut.ratio)));
        set(self.params.dut_usteps_per_rev, Value::I32(state.dut.usteps_per_rev.unwrap_or(0)));

        publish_axis(&self.bus, &self.params.axis_a, state.dut.axis(Axis::A));
        publish_axis(&self.bus, &self.params.axis_b, state.dut.axis(Axis::B));

        match state.dut.threshold {
            Some(threshold) => {
                set(self.params.threshold_floor, Value::I32(threshold.floor as i32));
                set(self.params.threshold_band, Value::I32(threshold.band as i32));
                set(self.params.threshold_applied, Value::I32(threshold.applied as i32));
                set(self.params.threshold_calibrated_at_s, Value::I32(threshold.calibrated_at_s));
            }
            // -1 is "never this session", and the page draws that as its own alarming state
            // rather than as a row of zeroes.
            None => set(self.params.threshold_calibrated_at_s, Value::I32(-1)),
        }

        set(self.params.faults_active, Value::I32(state.faults as i32));
        set(self.params.counts_passed, Value::I32(self.bench.passed as i32));
        set(self.params.counts_failed, Value::I32(self.bench.failed as i32));
        set(self.params.counts_aborted, Value::I32(self.bench.aborted as i32));

        let status = self.bench.run_status();
        set(self.params.run_busy, Value::Bool(status.is_some()));
        match &status {
            Some(status) => {
                text(self.params.run_plan, &status.plan);
                set(self.params.run_phase, Value::Enum(phase_value(status.phase)));
                set(self.params.run_origin, Value::Enum(origin_value(status.origin)));
                text(self.params.run_step_name, &status.step_name);
                set(self.params.run_step_index, Value::I32(status.step_index as i32));
                set(self.params.run_step_count, Value::I32(status.step_count as i32));
                set(
                    self.params.run_step_fraction,
                    Value::F64(match status.step_count {
                        0 => 0.0,
                        count => status.step_index as f64 / count as f64,
                    }),
                );
                set(self.params.run_cycle, Value::I32(status.cycle as i32));
                set(self.params.run_cycle_count, Value::I32(status.cycle_count as i32));
                set(self.params.run_elapsed_s, Value::I32(status.elapsed_s));
                // -1 is "unknown", and it stays that way until there is a real basis for an
                // estimate. A made-up ETA on a routine whose duration varies with how cold the
                // module is would be worse than none: people plan around it.
                set(self.params.run_eta_s, Value::I32(-1));
            }
            None => {
                set(self.params.run_phase, Value::Enum(phase_value(Phase::Idle)));
                set(self.params.run_origin, Value::Enum(0));
                text(self.params.run_step_name, "");
                set(self.params.run_step_index, Value::I32(0));
                set(self.params.run_step_count, Value::I32(0));
                set(self.params.run_step_fraction, Value::F64(0.0));
            }
        }
        *self.shared.run.lock().unwrap() = status;

        *self.shared.state.lock().unwrap() = state;
        self.publish_log(now);
    }

    fn publish_log(&mut self, _now: u64) {
        let lines: Vec<serde_json::Value> = self
            .bench
            .log()
            .since(self.log_cursor)
            .iter()
            .map(|line| {
                serde_json::json!({
                    "seq": line.seq,
                    "at_ms": line.at_ms,
                    "level": line.level,
                    "source": line.source,
                    "message": line.message,
                })
            })
            .collect();
        if lines.is_empty() {
            return;
        }
        self.log_cursor = self.bench.log().next_seq();
        let mut shared = self.shared.log.lock().unwrap();
        shared.extend(lines);
        // The bench's own ring is the scrollback; this mirror only needs to be bounded so a
        // long soak does not grow it without limit.
        let excess = shared.len().saturating_sub(bench_core::state::LOG_CAPACITY);
        if excess > 0 {
            shared.drain(..excess);
        }
    }

    fn publish_outcome(&mut self, outcome: &Outcome) {
        let set = |id, value| {
            let _ = self.bus.set(id, value);
        };
        let text = |id, value: &str| {
            let _ = self.bus.set_text(id, value);
        };

        set(self.params.last_verdict, Value::Enum(verdict_value(outcome.verdict.name())));
        text(self.params.last_plan, &outcome.plan);
        text(self.params.last_reason, &outcome.verdict.reason());
        text(
            self.params.last_advice,
            match &outcome.verdict {
                bench_core::verdict::Verdict::Error { advice, .. } => advice.as_deref().unwrap_or(""),
                _ => "",
            },
        );
        text(self.params.last_measurements, &outcome.summary());
        text(self.params.last_report_path, outcome.report_path.as_deref().unwrap_or(""));
        self.last_seq += 1;
        set(self.params.last_seq, Value::I64(self.last_seq));

        self.cue(match outcome.verdict {
            bench_core::verdict::Verdict::Pass => "pass",
            bench_core::verdict::Verdict::Fail { .. } => "fail",
            _ => "abort",
        });
    }
}

fn publish_axis(bus: &Bus, params: &AxisParams, state: &bench_core::state::AxisState) {
    let set = |id, value| {
        let _ = bus.set(id, value);
    };
    set(params.position, Value::I32(state.position.unwrap_or(0)));
    set(params.target, Value::I32(state.target.unwrap_or(0)));
    // A flag nobody has reported stays false on the bus; the page decides how to draw that
    // using `/dut/present`, so "not measured" and "measured and bad" stay distinguishable.
    set(params.position_known, Value::Bool(state.position.is_some()));
    set(params.health_known, Value::Bool(state.health.is_some()));
    let health = state.health.unwrap_or_default();
    set(params.health_home, Value::Bool(health.home_ok));
    set(params.health_switches, Value::Bool(health.switches_ok));
    set(params.health_backlash, Value::Bool(health.backlash_ok));
    set(params.health_measure_cycle, Value::Bool(health.measure_cycle_ok));
}

// --- enum mapping, by name in both directions ------------------------------------------

pub fn transport_from(value: u32) -> Option<LinkKind> {
    match schema::TRANSPORTS.iter().find(|(v, _)| *v == value).map(|(_, n)| *n) {
        Some("vcp") => Some(LinkKind::Vcp),
        Some("bench-ascii") => Some(LinkKind::BenchAscii),
        Some("rs485-serial") => Some(LinkKind::Rs485Serial),
        Some("rs485-tcp") => Some(LinkKind::Rs485Tcp),
        Some("sim") => Some(LinkKind::Sim),
        _ => None,
    }
}

fn by_name(table: &[(u32, &str)], name: &str) -> u32 {
    table.iter().find(|(_, n)| *n == name).map(|(v, _)| *v).unwrap_or(0)
}

fn transport_value(kind: LinkKind) -> u32 {
    by_name(schema::TRANSPORTS, kind.name())
}

fn firmware_value(kind: FirmwareKind) -> u32 {
    by_name(
        schema::FIRMWARE_KINDS,
        match kind {
            FirmwareKind::Unknown => "unknown",
            FirmwareKind::Production => "production",
            FirmwareKind::Bench => "bench",
            FirmwareKind::BootloaderOnly => "bootloader-only",
        },
    )
}

fn ratio_value(ratio: GearRatio) -> u32 {
    by_name(
        schema::GEAR_RATIOS,
        match ratio {
            GearRatio::Unknown => "unknown",
            GearRatio::R16 => "16",
            GearRatio::R32 => "32",
        },
    )
}

fn phase_value(phase: Phase) -> u32 {
    by_name(schema::RUN_PHASES, phase.name())
}

fn origin_value(origin: Origin) -> u32 {
    by_name(schema::RUN_ORIGINS, origin.name())
}

fn verdict_value(name: &str) -> u32 {
    by_name(schema::VERDICTS, name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every enum this worker publishes must survive the name round trip. Keying a page on a
    /// discriminant is the failure these tables exist to prevent, and a name that does not
    /// appear in its table silently becomes 0 — which is `none`, `unknown` or `idle`.
    #[test]
    fn every_enum_maps_to_a_declared_variant_by_name() {
        for kind in [
            LinkKind::Vcp,
            LinkKind::BenchAscii,
            LinkKind::Rs485Serial,
            LinkKind::Rs485Tcp,
            LinkKind::Sim,
        ] {
            let value = transport_value(kind);
            assert_eq!(transport_from(value), Some(kind), "{kind:?} did not round trip");
        }

        // A non-default variant must not collapse to 0.
        assert_ne!(firmware_value(FirmwareKind::Production), 0);
        assert_ne!(ratio_value(GearRatio::R32), 0);
        assert_ne!(phase_value(Phase::Body), 0);
        assert_ne!(origin_value(Origin::Agent), 0);
        assert_ne!(verdict_value("pass"), 0);
    }

    #[test]
    fn the_declared_action_list_and_the_resolved_one_are_the_same() {
        // `declare` and `resolve` both iterate `ACTIONS`, so this guards the list itself
        // against duplicates, which would make one action shadow another.
        let mut sorted = schema::ACTIONS.to_vec();
        sorted.sort_unstable();
        let count = sorted.len();
        sorted.dedup();
        assert_eq!(sorted.len(), count, "duplicate action names");
    }
}
