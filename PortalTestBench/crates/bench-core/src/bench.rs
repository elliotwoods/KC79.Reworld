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
use crate::state::{BenchState, ChannelState, LogRing, TelemetryRing};
use crate::transport::{Channel, Link, LinkError, LinkEvent, LinkKind, Op, RawSignal};
use crate::verdict::{Measurement, Verdict, summarise};

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
    serial_link: Option<Box<dyn Link>>,
    rs485_link: Option<Box<dyn Link>>,
    state: BenchState,
    log: LogRing,
    telemetry: TelemetryRing,
    engine: Option<Engine>,
    run: Option<RunMeta>,
    last: Option<Outcome>,
    queue: VecDeque<(Channel, Outbound)>,
    report: Report,
    started_ms: u64,
    next_full_poll_ms: u64,
    next_position_poll_ms: u64,
    runs: u64,
    pub passed: u32,
    pub failed: u32,
    pub aborted: u32,
}

#[derive(Debug)]
enum Outbound {
    Op(Op),
    Raw(RawSignal),
}

struct RunMeta {
    id: String,
    origin: Origin,
    started_ms: u64,
}

impl Bench {
    pub fn new(report: Report) -> Self {
        Self {
            serial_link: None,
            rs485_link: None,
            state: BenchState::default(),
            log: LogRing::default(),
            telemetry: TelemetryRing::default(),
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
    pub fn telemetry(&self) -> &TelemetryRing {
        &self.telemetry
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
        let channel = kind.channel();
        self.disconnect_channel(channel, now_ms);
        let mut link = crate::open_link(kind, endpoint)?;
        match link.open() {
            Ok(info) => {
                let state = self.channel_state_mut(channel);
                state.link.kind = Some(kind);
                state.link.endpoint = Some(info.endpoint.clone());
                state.link.connected = true;
                state.link.detail = None;
                if channel == Channel::Rs485 && state.selected_target.is_none() {
                    state.selected_target = Some(crate::transport::rs485::DEFAULT_TARGET);
                }
                match channel {
                    Channel::Serial => self.serial_link = Some(link),
                    Channel::Rs485 => self.rs485_link = Some(link),
                }
                self.state.active_channel = channel;
                self.sync_active();
                self.report
                    .device_connect(kind.name(), endpoint, true, None);
                self.note(
                    now_ms,
                    crate::LOG_LEVEL_STATUS,
                    format!("{} link open on {endpoint}", kind.name()),
                );
                // Ask immediately: production firmware says nothing until spoken to, so a
                // silent port would otherwise be indistinguishable from a dead one.
                self.queue.push_back((channel, Outbound::Op(Op::Identify)));
                Ok(())
            }
            Err(error) => {
                let detail = error.to_string();
                self.channel_state_mut(channel).link.detail = Some(detail.clone());
                self.sync_active();
                self.report
                    .device_connect(kind.name(), endpoint, false, Some(&detail));
                self.note(
                    now_ms,
                    crate::LOG_LEVEL_ERROR,
                    format!("could not open {endpoint}: {detail}"),
                );
                Err(detail)
            }
        }
    }

    /// Compatibility operation: close both communication lanes.
    pub fn disconnect(&mut self, now_ms: u64) {
        self.disconnect_channel(Channel::Serial, now_ms);
        self.disconnect_channel(Channel::Rs485, now_ms);
    }

    pub fn disconnect_channel(&mut self, channel: Channel, now_ms: u64) {
        let link = match channel {
            Channel::Serial => &mut self.serial_link,
            Channel::Rs485 => &mut self.rs485_link,
        };
        if let Some(link) = link.as_mut() {
            link.close();
            self.log.push(
                now_ms,
                crate::LOG_LEVEL_STATUS,
                "bench",
                format!("{} link closed", channel.name()),
            );
        }
        *link = None;
        *self.channel_state_mut(channel) = ChannelState::default();
        self.sync_active();
    }

    pub fn select_channel(&mut self, channel: Channel) {
        self.state.active_channel = channel;
        self.sync_active();
    }

    pub fn select_rs485_target(&mut self, target: i8) -> Result<(), String> {
        if !(1..=127).contains(&target) {
            return Err(format!("RS485 target must be 1..=127, got {target}"));
        }
        if let Some(link) = self.rs485_link.as_mut() {
            link.set_target(target).map_err(|error| error.to_string())?;
        }
        let state = &mut self.state.channels.rs485;
        state.selected_target = Some(target);
        state.dut = Default::default();
        if !state.discovered.contains(&target) {
            state.discovered.push(target);
            state.discovered.sort_unstable();
        }
        self.sync_active();
        Ok(())
    }

    pub fn discover_rs485(&mut self) {
        self.state.channels.rs485.discovered.clear();
        self.queue
            .push_back((Channel::Rs485, Outbound::Op(Op::Identify)));
    }

    /// Queue one op for the module.
    pub fn submit(&mut self, op: Op) {
        self.queue
            .push_back((self.state.active_channel, Outbound::Op(op)));
    }

    pub fn submit_to(&mut self, channel: Channel, op: Op) {
        self.queue.push_back((channel, Outbound::Op(op)));
    }

    pub fn submit_raw(&mut self, channel: Channel, signal: RawSignal, now_ms: u64) {
        let summary = match &signal {
            RawSignal::VcomText { text, ending } => {
                format!("VCOM raw {:?}: {text:?}", ending)
            }
            RawSignal::Rs485Json { body } => format!("RS485 raw: {body}"),
        };
        self.note(now_ms, crate::LOG_LEVEL_STATUS, summary);
        self.queue.push_back((channel, Outbound::Raw(signal)));
    }

    // --- runs ---------------------------------------------------------------------------

    /// Validate and start a plan. Refuses if one is already in flight.
    pub fn start(&mut self, plan: Plan, origin: Origin, now_ms: u64) -> Result<String, StartError> {
        if let Some(status) = self.run_status() {
            return Err(StartError::Busy {
                run_id: status.run_id,
                plan: status.plan,
            });
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
        self.note(
            now_ms,
            crate::LOG_LEVEL_STATUS,
            format!("{} started ({})", plan.name, origin.name()),
        );
        let baseline = self
            .channel_state(self.state.active_channel)
            .diagnostics
            .clone();
        self.engine = Some(Engine::new_with_transport(
            plan,
            now_ms,
            self.log.next_seq(),
            baseline,
        ));
        self.run = Some(RunMeta {
            id: run_id.clone(),
            origin,
            started_ms: now_ms,
        });
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

        self.poll_channel(Channel::Serial, now_ms);
        self.poll_channel(Channel::Rs485, now_ms);
        self.sync_active();

        self.schedule_polls(now_ms);

        let mut finished = None;
        if let Some(engine) = self.engine.as_mut() {
            let mut outbox = Vec::new();
            let mut tick = Tick {
                now_ms,
                state: &self.state,
                log: &self.log,
                outbox: &mut outbox,
            };
            let progress = engine.tick(&mut tick);
            for op in outbox {
                self.queue
                    .push_back((self.state.active_channel, Outbound::Op(op)));
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
        if !self.state.channels.rs485.link.connected {
            return;
        }
        if now_ms >= self.next_position_poll_ms {
            self.queue
                .push_back((Channel::Rs485, Outbound::Op(Op::PollPosition)));
            self.next_position_poll_ms = now_ms + POSITION_POLL_PERIOD_MS;
        }
        if now_ms >= self.next_full_poll_ms {
            self.queue
                .push_back((Channel::Rs485, Outbound::Op(Op::Poll)));
            self.next_full_poll_ms = now_ms + FULL_POLL_PERIOD_MS;
        }
    }

    fn drain_queue(&mut self, now_ms: u64) {
        while let Some((channel, outbound)) = self.queue.pop_front() {
            let link = match channel {
                Channel::Serial => self.serial_link.as_mut(),
                Channel::Rs485 => self.rs485_link.as_mut(),
            };
            let Some(link) = link else {
                self.note(
                    now_ms,
                    crate::LOG_LEVEL_WARNING,
                    format!("dropped {outbound:?}: no {} link", channel.name()),
                );
                continue;
            };
            let sent = match &outbound {
                Outbound::Op(op) => link.send(op),
                Outbound::Raw(signal) => link.send_raw(signal),
            };
            if let Err(error) = sent {
                let detail = match &error {
                    // Not a fault of the module: the plan asked for something this dialect
                    // cannot say. Validation should have caught it, so say so loudly.
                    LinkError::Unsupported { .. } => {
                        format!("{error} (this should have been caught by plan validation)")
                    }
                    other => other.to_string(),
                };
                self.state.faults += 1;
                self.note(
                    now_ms,
                    crate::LOG_LEVEL_ERROR,
                    format!("{}: {detail}", channel.name()),
                );
            }
        }
    }

    fn poll_channel(&mut self, channel: Channel, now_ms: u64) {
        let (events, connected, diagnostics) = {
            let link = match channel {
                Channel::Serial => self.serial_link.as_mut(),
                Channel::Rs485 => self.rs485_link.as_mut(),
            };
            let Some(link) = link else { return };
            let events = link.poll(now_ms);
            (events, link.is_open(), link.diagnostics())
        };
        let channel_state = self.channel_state_mut(channel);
        channel_state.link.connected = connected;
        channel_state.diagnostics = diagnostics;
        for event in events {
            self.apply_from(channel, event, now_ms);
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
    #[cfg(test)]
    fn apply(&mut self, event: LinkEvent, now_ms: u64) {
        self.apply_from(self.state.active_channel, event, now_ms);
    }

    fn apply_from(&mut self, channel: Channel, event: LinkEvent, now_ms: u64) {
        match event {
            LinkEvent::Identified {
                firmware,
                version,
                ratio,
                usteps_per_rev,
                banner,
            } => {
                let dut = {
                    let dut = &mut self.channel_state_mut(channel).dut;
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
                    dut.clone()
                };
                self.report.dut_identity(&dut);
                self.note(
                    now_ms,
                    crate::LOG_LEVEL_STATUS,
                    format!("{} identified: {banner}", channel.name()),
                );
            }
            LinkEvent::Position {
                axis,
                position,
                target,
            } => {
                let selected_target = self.channel_state(channel).selected_target;
                {
                    let dut = &mut self.channel_state_mut(channel).dut;
                    dut.present = true;
                    let axis_state = dut.axis_mut(axis);
                    axis_state.position = Some(position);
                    if target.is_some() {
                        axis_state.target = target;
                    }
                }
                self.telemetry
                    .push(now_ms, channel, selected_target, axis, position, target);
            }
            LinkEvent::HealthReport { axis, health } => {
                let dut = &mut self.channel_state_mut(channel).dut;
                dut.present = true;
                dut.axis_mut(axis).health = Some(health);
            }
            LinkEvent::Uptime { seconds } => {
                let dut = &mut self.channel_state_mut(channel).dut;
                dut.present = true;
                dut.uptime_s = Some(seconds);
            }
            LinkEvent::Log {
                level,
                message,
                firmware_ms,
            } => {
                self.channel_state_mut(channel).dut.present = true;
                self.log
                    .push(now_ms, level, channel.name(), message.clone());
                self.report.firmware_log(level, &message, firmware_ms);
            }
            LinkEvent::Sensor { active, threshold } => {
                // Streamed at 60 Hz by the bench firmware; it belongs in telemetry, not the log.
                let _ = (active, threshold);
                self.channel_state_mut(channel).dut.present = true;
            }
            LinkEvent::Token { kind, fields } => {
                self.report.token(kind, &fields);
            }
            LinkEvent::PeerSeen { source } => {
                if channel == Channel::Rs485 && source > 0 {
                    let found = &mut self.state.channels.rs485.discovered;
                    if !found.contains(&source) {
                        found.push(source);
                        found.sort_unstable();
                    }
                }
            }
            LinkEvent::Ack { .. } => {}
            LinkEvent::Fault(detail) => {
                self.state.faults += 1;
                self.log.push(
                    now_ms,
                    crate::LOG_LEVEL_ERROR,
                    "fault",
                    format!("{}: {detail}", channel.name()),
                );
                self.report.fault(&detail);
            }
        }
        self.sync_active();
    }

    fn channel_state(&self, channel: Channel) -> &ChannelState {
        match channel {
            Channel::Serial => &self.state.channels.serial,
            Channel::Rs485 => &self.state.channels.rs485,
        }
    }

    fn channel_state_mut(&mut self, channel: Channel) -> &mut ChannelState {
        match channel {
            Channel::Serial => &mut self.state.channels.serial,
            Channel::Rs485 => &mut self.state.channels.rs485,
        }
    }

    fn sync_active(&mut self) {
        let active = self.channel_state(self.state.active_channel).clone();
        self.state.link = active.link;
        self.state.dut = active.dut;
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
        bench.state.dut.threshold = Some(crate::state::ThresholdState::from_band(
            crate::threshold::Band::new(240, 252).unwrap(),
            0,
        ));

        let plan = Plan {
            name: "first".into(),
            body: Body::Once(vec![Step::Poll]),
            ..Plan::default()
        };
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
            body: Body::Once(vec![Step::Home {
                axis: crate::dut::Axis::A,
            }]),
            ..Plan::default()
        };
        let error = bench.start(plan, Origin::Cli, 0).unwrap_err();
        assert!(matches!(
            error,
            StartError::Invalid(PlanError::UncalibratedHome { .. })
        ));
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
            LinkEvent::Log {
                level: 20,
                message: "[E Routines.init] Fail".into(),
                firmware_ms: None,
            },
            5,
        );
        let lines = bench.log().since(0);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].level, 20);
        assert_eq!(lines[0].source, "serial");
    }

    #[test]
    fn serial_and_rs485_observations_stay_independent() {
        let mut bench = bench();
        bench.apply_from(
            Channel::Serial,
            LinkEvent::Identified {
                firmware: FirmwareKind::Bench,
                version: Some("bench-7".into()),
                ratio: crate::dut::GearRatio::R32,
                usteps_per_rev: Some(189_704),
                banner: "bench".into(),
            },
            1,
        );
        bench.apply_from(
            Channel::Rs485,
            LinkEvent::Identified {
                firmware: FirmwareKind::Production,
                version: Some("prod-9".into()),
                ratio: crate::dut::GearRatio::Unknown,
                usteps_per_rev: None,
                banner: "production".into(),
            },
            2,
        );

        assert_eq!(
            bench.state.channels.serial.dut.version.as_deref(),
            Some("bench-7")
        );
        assert_eq!(
            bench.state.channels.rs485.dut.version.as_deref(),
            Some("prod-9")
        );
        bench.select_channel(Channel::Rs485);
        assert_eq!(bench.state.dut.version.as_deref(), Some("prod-9"));
        bench.select_channel(Channel::Serial);
        assert_eq!(bench.state.dut.version.as_deref(), Some("bench-7"));
    }

    #[test]
    fn discovery_is_sorted_deduplicated_and_never_treats_broadcast_as_a_peer() {
        let mut bench = bench();
        for source in [7, 2, 7, -1, 1] {
            bench.apply_from(Channel::Rs485, LinkEvent::PeerSeen { source }, 0);
        }
        assert_eq!(bench.state.channels.rs485.discovered, vec![1, 2, 7]);
        assert!(bench.state.channels.serial.discovered.is_empty());
    }

    #[test]
    fn measured_motion_records_its_channel_axis_and_selected_peer() {
        let mut bench = bench();
        bench.state.channels.rs485.selected_target = Some(12);
        bench.apply_from(
            Channel::Rs485,
            LinkEvent::Position {
                axis: crate::dut::Axis::B,
                position: 42,
                target: Some(50),
            },
            123,
        );
        let samples = bench.telemetry().since(0);
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].channel, Channel::Rs485);
        assert_eq!(samples[0].target_id, Some(12));
        assert_eq!(samples[0].axis, crate::dut::Axis::B);
        assert_eq!(samples[0].position, 42);
    }

    #[test]
    fn aborting_with_nothing_running_reports_that_rather_than_pretending() {
        let mut bench = bench();
        assert!(!bench.abort(0));
    }
}
