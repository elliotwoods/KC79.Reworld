//! The bench itself: one link, one module, one run at a time.
//!
//! This is what both front doors drive. The operator app's worker thread owns a `Bench` and
//! mirrors it into bus parameters; `ptb --local` owns one directly. Neither has its own idea of
//! what a run is, which is what stops the GUI and the CLI disagreeing about the hardware in
//! front of them.
//!
//! **The single poller lives here.** [`Bench::tick`] is the only place [`Link::poll`] is
//! called; everything else reads [`Bench::state`] and [`Bench::log`]. See the transport module
//! docs for why that is a correctness property rather than a tidiness one.

use std::collections::VecDeque;

use crate::dut::FirmwareKind;
use crate::engine::{Engine, Phase, Progress, Tick};
use crate::plan::{Plan, PlanError, ValidationContext};
use crate::report::Report;
use crate::state::{BenchState, LogRing};
use crate::transport::{Link, LinkError, LinkEvent, LinkKind, Op};
use crate::verdict::{summarise, Measurement, Verdict};

/// How often the bench asks a connected module for a full status report.
///
/// Deliberately slow. A full poll **drains the firmware's log outbox**, and those lines exist
/// nowhere else afterwards, so polling fast would shred the log into fragments spread across
/// replies. Live position comes from the cheap position poll instead.
pub const FULL_POLL_PERIOD_MS: u64 = 2_000;

/// How often positions are refreshed while a module is connected.
pub const POSITION_POLL_PERIOD_MS: u64 = 200;

/// Who asked for the run in flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Origin {
    #[default]
    None,
    Gui,
    Agent,
    Cli,
}

impl Origin {
    pub fn name(self) -> &'static str {
        match self {
            Origin::None => "none",
            Origin::Gui => "gui",
            Origin::Agent => "agent",
            Origin::Cli => "cli",
        }
    }
}

/// The answer from a finished run.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub run_id: String,
    pub plan: String,
    pub origin: Origin,
    pub verdict: Verdict,
    pub measurements: Vec<Measurement>,
    pub duration_ms: u64,
    pub report_path: Option<String>,
}

impl Outcome {
    pub fn summary(&self) -> String {
        summarise(&self.measurements)
    }
}

/// What a run currently looks like, for the progress display.
#[derive(Debug, Clone)]
pub struct RunStatus {
    pub run_id: String,
    pub plan: String,
    pub origin: Origin,
    pub phase: Phase,
    pub step_name: String,
    pub step_index: usize,
    pub step_count: usize,
    pub cycle: u32,
    pub cycle_count: u32,
    pub elapsed_s: i32,
}

pub struct Bench {
    link: Option<Box<dyn Link>>,
    state: BenchState,
    log: LogRing,
    engine: Option<Engine>,
    run: Option<RunMeta>,
    last: Option<Outcome>,
    queue: VecDeque<Op>,
    report: Report,
    started_ms: u64,
    next_full_poll_ms: u64,
    next_position_poll_ms: u64,
    runs: u64,
    pub passed: u32,
    pub failed: u32,
    pub aborted: u32,
}

struct RunMeta {
    id: String,
    origin: Origin,
    started_ms: u64,
}

impl Bench {
    pub fn new(report: Report) -> Self {
        Self {
            link: None,
            state: BenchState::default(),
            log: LogRing::default(),
            engine: None,
            run: None,
            last: None,
            queue: VecDeque::new(),
            report,
            started_ms: 0,
            next_full_poll_ms: 0,
            next_position_poll_ms: 0,
            runs: 0,
            passed: 0,
            failed: 0,
            aborted: 0,
        }
    }

    pub fn state(&self) -> &BenchState {
        &self.state
    }
    pub fn log(&self) -> &LogRing {
        &self.log
    }
    pub fn last_outcome(&self) -> Option<&Outcome> {
        self.last.as_ref()
    }
    pub fn is_busy(&self) -> bool {
        self.engine.is_some()
    }

    pub fn run_status(&self) -> Option<RunStatus> {
        let engine = self.engine.as_ref()?;
        let meta = self.run.as_ref()?;
        Some(RunStatus {
            run_id: meta.id.clone(),
            plan: engine.plan().name.clone(),
            origin: meta.origin,
            phase: engine.phase(),
            step_name: engine.step_name(),
            step_index: engine.step_index(),
            step_count: engine.step_count(),
            cycle: engine.cycle(),
            cycle_count: engine.cycle_count(),
            elapsed_s: engine.elapsed_s(self.started_ms.max(meta.started_ms)),
        })
    }

    /// Record a line in the bench's own voice, and in the session file.
    pub fn note(&mut self, now_ms: u64, level: u8, message: impl Into<String>) {
        let message = message.into();
        self.log.push(now_ms, level, "bench", message.clone());
        self.report.log(level, "bench", &message);
    }

    // --- link ---------------------------------------------------------------------------

    pub fn connect(&mut self, kind: LinkKind, endpoint: &str, now_ms: u64) -> Result<(), String> {
        self.disconnect(now_ms);
        let mut link = crate::open_link(kind, endpoint)?;
        match link.open() {
            Ok(info) => {
                self.state.link.kind = Some(kind);
                self.state.link.endpoint = Some(info.endpoint.clone());
                self.state.link.connected = true;
                self.state.link.detail = None;
                self.link = Some(link);
                self.report.device_connect(kind.name(), endpoint, true, None);
                self.note(now_ms, crate::LOG_LEVEL_STATUS, format!("{} link open on {endpoint}", kind.name()));
                // Ask immediately: production firmware says nothing until spoken to, so a
                // silent port would otherwise be indistinguishable from a dead one.
                self.queue.push_back(Op::Identify);
                Ok(())
            }
            Err(error) => {
                let detail = error.to_string();
                self.state.link.detail = Some(detail.clone());
                self.report.device_connect(kind.name(), endpoint, false, Some(&detail));
                self.note(now_ms, crate::LOG_LEVEL_ERROR, format!("could not open {endpoint}: {detail}"));
                Err(detail)
            }
        }
    }

    pub fn disconnect(&mut self, now_ms: u64) {
        if let Some(link) = self.link.as_mut() {
            link.close();
            self.note(now_ms, crate::LOG_LEVEL_STATUS, "link closed");
        }
        self.link = None;
        self.state.link.connected = false;
        // What the module *was* is not what it *is*. Clearing this is what stops the page
        // showing a confident identity for something no longer attached.
        self.state.dut = Default::default();
    }

    /// Queue one op for the module.
    pub fn submit(&mut self, op: Op) {
        self.queue.push_back(op);
    }

    // --- runs ---------------------------------------------------------------------------

    /// Validate and start a plan. Refuses if one is already in flight.
    pub fn start(&mut self, plan: Plan, origin: Origin, now_ms: u64) -> Result<String, StartError> {
        if let Some(status) = self.run_status() {
            return Err(StartError::Busy { run_id: status.run_id, plan: status.plan });
        }

        let context = ValidationContext {
            transport: self.state.link.kind.unwrap_or(LinkKind::Sim),
            firmware: self.state.dut.firmware,
            ratio: self.state.dut.ratio,
            // Only the mechanical build has no optical threshold to establish. An unidentified
            // module is treated as optical, because assuming the permissive case is how a home
            // ends up running on a threshold nobody measured.
            optical: self.state.dut.firmware != FirmwareKind::Bench,
            threshold_calibrated: self.state.dut.threshold.is_some(),
        };
        plan.validate(&context).map_err(StartError::Invalid)?;

        self.runs += 1;
        let run_id = format!("r-{:04}", self.runs);
        self.report.plan_start(&run_id, &plan, origin.name());
        self.note(now_ms, crate::LOG_LEVEL_STATUS, format!("{} started ({})", plan.name, origin.name()));
        self.engine = Some(Engine::new(plan, now_ms, self.log.next_seq()));
        self.run = Some(RunMeta { id: run_id.clone(), origin, started_ms: now_ms });
        Ok(run_id)
    }

    /// Ask the run to stop. It escapes the routine and runs teardown first.
    pub fn abort(&mut self, now_ms: u64) -> bool {
        match self.engine.as_mut() {
            Some(engine) => {
                engine.cancel();
                self.note(now_ms, crate::LOG_LEVEL_WARNING, "abort requested");
                true
            }
            None => false,
        }
    }

    // --- the tick -----------------------------------------------------------------------

    /// Poll the link, apply what it said, advance the run, and send whatever is queued.
    ///
    /// The **only** caller of [`Link::poll`].
    pub fn tick(&mut self, now_ms: u64) -> Option<Outcome> {
        self.started_ms = now_ms;

        if let Some(link) = self.link.as_mut() {
            let events = link.poll(now_ms);
            let connected = link.is_open();
            self.state.link.connected = connected;
            for event in events {
                self.apply(event, now_ms);
            }
        }

        self.schedule_polls(now_ms);

        let mut finished = None;
        if let Some(engine) = self.engine.as_mut() {
            let mut outbox = Vec::new();
            let mut tick =
                Tick { now_ms, state: &self.state, log: &self.log, outbox: &mut outbox };
            let progress = engine.tick(&mut tick);
            for op in outbox {
                self.queue.push_back(op);
            }
            if let Progress::Done(verdict) = progress {
                finished = Some(verdict);
            }
        }

        if let Some(verdict) = finished {
            return Some(self.complete(verdict, now_ms));
        }

        self.drain_queue(now_ms);
        None
    }

    fn schedule_polls(&mut self, now_ms: u64) {
        if !self.state.link.connected {
            return;
        }
        // Only RS485 has structured polls; the line dialects stream or answer keystrokes.
        if !matches!(self.state.link.kind, Some(LinkKind::Rs485Serial) | Some(LinkKind::Rs485Tcp)) {
            return;
        }
        if now_ms >= self.next_position_poll_ms {
            self.queue.push_back(Op::PollPosition);
            self.next_position_poll_ms = now_ms + POSITION_POLL_PERIOD_MS;
        }
        if now_ms >= self.next_full_poll_ms {
            self.queue.push_back(Op::Poll);
            self.next_full_poll_ms = now_ms + FULL_POLL_PERIOD_MS;
        }
    }

    fn drain_queue(&mut self, now_ms: u64) {
        while let Some(op) = self.queue.pop_front() {
            let Some(link) = self.link.as_mut() else {
                self.note(now_ms, crate::LOG_LEVEL_WARNING, format!("dropped {op:?}: no link"));
                continue;
            };
            if let Err(error) = link.send(&op) {
                let detail = match &error {
                    // Not a fault of the module: the plan asked for something this dialect
                    // cannot say. Validation should have caught it, so say so loudly.
                    LinkError::Unsupported { .. } => format!("{error} (this should have been caught by plan validation)"),
                    other => other.to_string(),
                };
                self.state.faults += 1;
                self.note(now_ms, crate::LOG_LEVEL_ERROR, detail);
            }
        }
    }

    fn complete(&mut self, verdict: Verdict, now_ms: u64) -> Outcome {
        let engine = self.engine.take().expect("a run was in flight");
        let meta = self.run.take().expect("a run was in flight");

        match &verdict {
            Verdict::Pass => self.passed += 1,
            Verdict::Fail { .. } => self.failed += 1,
            Verdict::Aborted { .. } | Verdict::Error { .. } => self.aborted += 1,
        }

        let outcome = Outcome {
            run_id: meta.id.clone(),
            plan: engine.plan().name.clone(),
            origin: meta.origin,
            verdict: verdict.clone(),
            measurements: engine.measurements().to_vec(),
            duration_ms: now_ms.saturating_sub(meta.started_ms),
            report_path: self.report.path().map(|p| p.to_string_lossy().into_owned()),
        };

        self.report.verdict(&outcome);
        let level = match verdict {
            Verdict::Pass => crate::LOG_LEVEL_STATUS,
            _ => crate::LOG_LEVEL_ERROR,
        };
        let reason = outcome.verdict.reason();
        let message = match reason.is_empty() {
            true => format!("{} {}", outcome.plan, outcome.verdict.name()),
            false => format!("{} {}: {reason}", outcome.plan, outcome.verdict.name()),
        };
        self.note(now_ms, level, message);

        self.last = Some(outcome.clone());
        outcome
    }

    /// Fold one thing the module said into what we believe.
    fn apply(&mut self, event: LinkEvent, now_ms: u64) {
        match event {
            LinkEvent::Identified { firmware, version, ratio, usteps_per_rev, banner } => {
                let dut = &mut self.state.dut;
                dut.present = true;
                dut.firmware = firmware;
                if version.is_some() {
                    dut.version = version;
                }
                // Never downgrade a known gearing to unknown: the RS485 report does not carry
                // it, so a later poll must not erase what the home routine established.
                if ratio != crate::dut::GearRatio::Unknown {
                    dut.ratio = ratio;
                }
                if usteps_per_rev.is_some() {
                    dut.usteps_per_rev = usteps_per_rev;
                }
                dut.banner = Some(banner.clone());
                self.report.dut_identity(dut);
                self.note(now_ms, crate::LOG_LEVEL_STATUS, format!("identified: {banner}"));
            }
            LinkEvent::Position { axis, position, target } => {
                self.state.dut.present = true;
                let axis_state = self.state.dut.axis_mut(axis);
                axis_state.position = Some(position);
                if target.is_some() {
                    axis_state.target = target;
                }
            }
            LinkEvent::HealthReport { axis, health } => {
                self.state.dut.present = true;
                self.state.dut.axis_mut(axis).health = Some(health);
            }
            LinkEvent::Uptime { seconds } => {
                self.state.dut.present = true;
                self.state.dut.uptime_s = Some(seconds);
            }
            LinkEvent::Log { level, message, firmware_ms } => {
                self.state.dut.present = true;
                self.log.push(now_ms, level, "firmware", message.clone());
                self.report.firmware_log(level, &message, firmware_ms);
            }
            LinkEvent::Sensor { active, threshold } => {
                // Streamed at 60 Hz by the bench firmware; it belongs in telemetry, not the log.
                let _ = (active, threshold);
                self.state.dut.present = true;
            }
            LinkEvent::Token { kind, fields } => {
                self.report.token(kind, &fields);
            }
            LinkEvent::Ack => {}
            LinkEvent::Fault(detail) => {
                self.state.faults += 1;
                self.log.push(now_ms, crate::LOG_LEVEL_ERROR, "fault", detail.clone());
                self.report.fault(&detail);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StartError {
    #[error("{plan} is already running as {run_id}")]
    Busy { run_id: String, plan: String },

    #[error("{0}")]
    Invalid(#[from] PlanError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Body, Step};

    fn bench() -> Bench {
        Bench::new(Report::disabled())
    }

    #[test]
    fn a_second_run_is_refused_by_name_rather_than_queued() {
        let mut bench = bench();
        bench.state.link.connected = true;
        bench.state.link.kind = Some(LinkKind::Sim);
        bench.state.dut.threshold =
            Some(crate::state::ThresholdState::from_band(crate::threshold::Band::new(240, 252).unwrap(), 0));

        let plan = Plan { name: "first".into(), body: Body::Once(vec![Step::Poll]), ..Plan::default() };
        let id = bench.start(plan.clone(), Origin::Gui, 0).unwrap();

        let error = bench.start(plan, Origin::Agent, 0).unwrap_err();
        assert!(matches!(&error, StartError::Busy { run_id, .. } if *run_id == id));
    }

    /// The invariant, enforced where it counts: `start` refuses, so the motor never turns.
    #[test]
    fn starting_a_plan_that_homes_without_a_threshold_is_refused() {
        let mut bench = bench();
        bench.state.link.connected = true;
        bench.state.link.kind = Some(LinkKind::Sim);
        bench.state.dut.firmware = FirmwareKind::Production;

        let plan = Plan {
            name: "home".into(),
            body: Body::Once(vec![Step::Home { axis: crate::dut::Axis::A }]),
            ..Plan::default()
        };
        let error = bench.start(plan, Origin::Cli, 0).unwrap_err();
        assert!(matches!(error, StartError::Invalid(PlanError::UncalibratedHome { .. })));
    }

    /// Disconnecting must clear the module's identity. Leaving it behind is how a page ends up
    /// confidently describing something that is no longer plugged in.
    #[test]
    fn disconnecting_forgets_what_was_attached() {
        let mut bench = bench();
        bench.apply(
            LinkEvent::Identified {
                firmware: FirmwareKind::Production,
                version: Some("v1".into()),
                ratio: crate::dut::GearRatio::R32,
                usteps_per_rev: Some(189_704),
                banner: "Portal v1".into(),
            },
            0,
        );
        assert!(bench.state.dut.present);

        bench.disconnect(1);
        assert!(!bench.state.dut.present);
        assert_eq!(bench.state.dut.version, None);
        assert!(!bench.state.link.connected);
    }

    /// The RS485 report does not carry the gearing, so a later poll must not erase what an
    /// earlier home routine established.
    #[test]
    fn a_later_report_does_not_erase_a_known_gearing() {
        let mut bench = bench();
        bench.apply(
            LinkEvent::Identified {
                firmware: FirmwareKind::Bench,
                version: None,
                ratio: crate::dut::GearRatio::R16,
                usteps_per_rev: Some(92_252),
                banner: "#".into(),
            },
            0,
        );
        bench.apply(
            LinkEvent::Identified {
                firmware: FirmwareKind::Production,
                version: Some("v2".into()),
                ratio: crate::dut::GearRatio::Unknown,
                usteps_per_rev: None,
                banner: "Portal v2".into(),
            },
            1,
        );
        assert_eq!(bench.state.dut.ratio, crate::dut::GearRatio::R16);
        assert_eq!(bench.state.dut.usteps_per_rev, Some(92_252));
    }

    #[test]
    fn firmware_log_lines_reach_the_ring() {
        let mut bench = bench();
        bench.apply(
            LinkEvent::Log { level: 20, message: "[E Routines.init] Fail".into(), firmware_ms: None },
            5,
        );
        let lines = bench.log().since(0);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].level, 20);
        assert_eq!(lines[0].source, "firmware");
    }

    #[test]
    fn aborting_with_nothing_running_reports_that_rather_than_pretending() {
        let mut bench = bench();
        assert!(!bench.abort(0));
    }
}
