//! Running a plan.
//!
//! # Tick-driven, never blocking
//!
//! [`Engine::tick`] is called once per worker tick and returns immediately. Nothing about a run
//! lives in a stack frame, an HTTP request or a browser — which is what lets an eight-hour soak
//! survive a page reload, a `ptb` disconnect, and a second GUI window opening.
//!
//! # Waiting for routines that acknowledge instantly
//!
//! The firmware ACKs a routine and *then* runs it for seconds to minutes. There is no completion
//! packet. So completion is detected by watching the log for the line the routine prints when it
//! finishes — `| END Routines.init` — which is why [`Step::AwaitLog`] exists and why it also
//! takes a `fail_if`: the firmware announces its failures too, and there is no reason to sit
//! out a four-minute timeout after it has already said no.
//!
//! # Abort never kills
//!
//! [`Engine::cancel`] sets a flag. The engine finishes the tick it is in, runs `teardown`
//! unconditionally — which is where the escape and the restore live — and only then reports
//! [`Verdict::Aborted`]. Teardown runs on **every** exit path, including an error.

use crate::plan::{Body, HealthFlag, Plan, Source, Step, TransportCounter, Until};
use crate::state::{BenchState, LogRing};
use crate::transport::{Channel, LinkDiagnostics, Op};
use crate::verdict::{MeasureValue, Measurement, Verdict, evaluate};

/// What the engine is doing, for the progress display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    #[default]
    Idle,
    Setup,
    Body,
    Teardown,
    /// Aborting: the escape has been sent and teardown is running.
    Escaping,
    Done,
}

impl Phase {
    pub fn name(self) -> &'static str {
        match self {
            Phase::Idle => "idle",
            Phase::Setup => "setup",
            Phase::Body => "body",
            Phase::Teardown => "teardown",
            Phase::Escaping => "escaping",
            Phase::Done => "done",
        }
    }
}

/// What the caller must supply each tick.
pub struct Tick<'a> {
    pub now_ms: u64,
    pub state: &'a BenchState,
    pub log: &'a LogRing,
    /// Ops the engine wants sent. The caller owns the link and does the sending.
    pub outbox: &'a mut Vec<Op>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Progress {
    Running,
    Done(Verdict),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    BeginBootloaderUpload { timeout_s: u64 },
}

/// One run of one plan.
pub struct Engine {
    plan: Plan,
    phase: Phase,
    /// Index within the current list.
    index: usize,
    cycle: u32,
    started_ms: u64,
    step_started_ms: u64,
    /// Where the log ring stood when the run began, so `LogSeen` and friends only look at
    /// lines this run produced.
    log_start_seq: u64,
    measurements: Vec<Measurement>,
    /// Set when a step decided the run has failed; teardown still runs before it is reported.
    pending: Option<Verdict>,
    cancelled: bool,
    /// True once the current step's op has been queued, so a tick that waits does not resend.
    dispatched: bool,
    transport_baseline: LinkDiagnostics,
    effects: Vec<Effect>,
    field_update_baseline: u64,
}

impl Engine {
    pub fn new(plan: Plan, now_ms: u64, log_seq: u64) -> Self {
        Self::new_with_transport(plan, now_ms, log_seq, LinkDiagnostics::default())
    }

    pub fn new_with_transport(
        plan: Plan,
        now_ms: u64,
        log_seq: u64,
        transport_baseline: LinkDiagnostics,
    ) -> Self {
        Self {
            plan,
            phase: Phase::Setup,
            index: 0,
            cycle: 1,
            started_ms: now_ms,
            step_started_ms: now_ms,
            log_start_seq: log_seq,
            measurements: Vec::new(),
            pending: None,
            cancelled: false,
            dispatched: false,
            transport_baseline,
            effects: Vec::new(),
            field_update_baseline: 0,
        }
    }

    pub fn plan(&self) -> &Plan {
        &self.plan
    }
    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn cycle(&self) -> u32 {
        self.cycle
    }
    pub fn measurements(&self) -> &[Measurement] {
        &self.measurements
    }

    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    /// Total cycles, when the plan is a soak with a fixed count.
    pub fn cycle_count(&self) -> u32 {
        match &self.plan.body {
            Body::Repeat {
                until: Until::Cycles(n),
                ..
            } => *n,
            _ => 1,
        }
    }

    pub fn elapsed_s(&self, now_ms: u64) -> i32 {
        ((now_ms.saturating_sub(self.started_ms)) / 1000) as i32
    }

    /// Ask the run to stop. It will not stop instantly; see the module docs.
    pub fn cancel(&mut self) {
        if !self.cancelled {
            self.cancelled = true;
        }
    }

    /// The list currently being executed.
    fn list(&self) -> &[Step] {
        match self.phase {
            Phase::Setup => &self.plan.setup,
            Phase::Body => self.plan.body.steps(),
            Phase::Teardown | Phase::Escaping => &self.plan.teardown,
            Phase::Idle | Phase::Done => &[],
        }
    }

    pub fn current_step(&self) -> Option<&Step> {
        self.list().get(self.index)
    }

    pub fn step_name(&self) -> String {
        self.current_step().map(Step::name).unwrap_or_default()
    }

    pub fn step_index(&self) -> usize {
        self.index
    }

    pub fn step_count(&self) -> usize {
        self.list().len()
    }

    pub fn tick(&mut self, tick: &mut Tick<'_>) -> Progress {
        // The overall limit is checked above the per-step deadlines, so a plan cannot outlive
        // its budget by accumulating steps that each stay inside theirs.
        if self.pending.is_none()
            && !matches!(self.phase, Phase::Teardown | Phase::Escaping | Phase::Done)
            && tick.now_ms.saturating_sub(self.started_ms) > self.plan.limits.max_duration_s * 1000
        {
            self.fail(Verdict::Aborted {
                reason: format!(
                    "plan exceeded its {} s limit",
                    self.plan.limits.max_duration_s
                ),
            });
        }

        let field_update_active =
            matches!(self.current_step(), Some(Step::BootloaderUpload { .. }))
                && tick.state.field_update.running();
        if self.cancelled
            && !field_update_active
            && !matches!(self.phase, Phase::Teardown | Phase::Escaping | Phase::Done)
        {
            self.pending.get_or_insert(Verdict::Aborted {
                reason: "stopped by the operator".into(),
            });
            // Escape first, then let teardown restore whatever the plan changed.
            tick.outbox.push(Op::Escape);
            self.enter(Phase::Escaping, tick.now_ms);
        }

        if self.phase == Phase::Done {
            return Progress::Done(self.finish());
        }

        // One step per tick at most: a step that completes leaves the next one to the next
        // tick, so the link always gets a chance to answer in between.
        match self.step(tick) {
            StepResult::Waiting => Progress::Running,
            StepResult::Advance => {
                self.advance(tick.now_ms);
                Progress::Running
            }
            StepResult::Failed(verdict) => {
                self.fail(verdict);
                Progress::Running
            }
        }
    }

    fn fail(&mut self, verdict: Verdict) {
        self.pending.get_or_insert(verdict);
        if !matches!(self.phase, Phase::Teardown | Phase::Escaping) {
            // Teardown still runs. A plan that changed the current or the microstep resolution
            // must put them back even when it is failing.
            self.phase = Phase::Teardown;
            self.index = 0;
            self.dispatched = false;
        }
    }

    fn enter(&mut self, phase: Phase, now_ms: u64) {
        self.phase = phase;
        self.index = 0;
        self.dispatched = false;
        self.step_started_ms = now_ms;
    }

    fn advance(&mut self, now_ms: u64) {
        self.index += 1;
        self.dispatched = false;
        self.step_started_ms = now_ms;

        if self.index < self.list().len() {
            return;
        }

        match self.phase {
            Phase::Setup => self.enter(Phase::Body, now_ms),
            Phase::Body => {
                let more = match &self.plan.body {
                    Body::Once(_) => false,
                    Body::Repeat { until, .. } => match until {
                        Until::Cycles(n) => self.cycle < *n,
                        Until::DurationS(seconds) => {
                            now_ms.saturating_sub(self.started_ms) < seconds * 1000
                        }
                    },
                };
                if more && self.pending.is_none() {
                    self.cycle += 1;
                    self.index = 0;
                    self.dispatched = false;
                } else {
                    self.enter(Phase::Teardown, now_ms);
                }
            }
            Phase::Teardown | Phase::Escaping => self.phase = Phase::Done,
            Phase::Idle | Phase::Done => {}
        }
    }

    fn finish(&mut self) -> Verdict {
        self.pending
            .take()
            .unwrap_or_else(|| evaluate(&self.plan.criteria, &self.measurements))
    }

    fn step(&mut self, tick: &mut Tick<'_>) -> StepResult {
        let Some(step) = self.current_step().cloned() else {
            return StepResult::Advance;
        };

        match &step {
            Step::Connect => {
                if tick.state.link.connected {
                    StepResult::Advance
                } else if tick.now_ms.saturating_sub(self.step_started_ms) > 5_000 {
                    StepResult::Failed(Verdict::Error {
                        detail: "no link to the module".into(),
                        advice: Some("choose a transport and connect before running a plan".into()),
                    })
                } else {
                    StepResult::Waiting
                }
            }

            Step::Sleep { ms } => {
                if tick.now_ms.saturating_sub(self.step_started_ms) >= *ms {
                    StepResult::Advance
                } else {
                    StepResult::Waiting
                }
            }

            Step::Marker { label } => {
                // Markers exist so a person reading the session file later can find the moment
                // they cared about. They are recorded, not just displayed.
                self.measurements.push(Measurement {
                    name: format!("marker:{label}"),
                    value: MeasureValue::Text(label.clone()),
                    unit: None,
                    at_ms: tick.now_ms.saturating_sub(self.started_ms),
                    step_index: self.index,
                });
                StepResult::Advance
            }

            Step::AwaitLog {
                contains,
                timeout_s,
                fail_if,
            } => {
                let lines = tick.log.since(self.log_start_seq);
                if let Some(fail_text) = fail_if
                    && let Some(hit) = lines
                        .iter()
                        .find(|l| l.message.contains(fail_text.as_str()))
                {
                    return StepResult::Failed(Verdict::Fail {
                        criterion: step.name(),
                        got: hit.message.clone(),
                        expected: format!("no line containing \"{fail_text}\""),
                    });
                }
                if lines.iter().any(|l| l.message.contains(contains.as_str())) {
                    return StepResult::Advance;
                }
                if tick.now_ms.saturating_sub(self.step_started_ms) > timeout_s * 1000 {
                    return StepResult::Failed(Verdict::Fail {
                        criterion: step.name(),
                        got: format!("nothing matched in {timeout_s} s"),
                        expected: format!("a log line containing \"{contains}\""),
                    });
                }
                StepResult::Waiting
            }

            Step::Record { name, from } => {
                let value = self.read(from, tick);
                self.measurements.push(Measurement {
                    name: name.clone(),
                    value,
                    unit: unit_for(from),
                    at_ms: tick.now_ms.saturating_sub(self.started_ms),
                    step_index: self.index,
                });
                StepResult::Advance
            }

            // Threshold calibration is a routine on the module, not something the host computes:
            // the census has to be read by the moving comparator. Until that is wired to a
            // transport, refuse rather than pretend a threshold was measured.
            Step::CalibrateThreshold { .. } => StepResult::Failed(Verdict::Error {
                detail: "threshold calibration is not wired to a transport yet".into(),
                advice: Some(
                    "run the plan against a module whose threshold is already calibrated, or set \
                     it explicitly with a set_home_threshold step"
                        .into(),
                ),
            }),

            Step::BootloaderUpload { timeout_s } => {
                if !self.dispatched {
                    self.field_update_baseline = tick.state.field_update.attempt;
                    self.effects.push(Effect::BeginBootloaderUpload {
                        timeout_s: *timeout_s,
                    });
                    self.dispatched = true;
                    return StepResult::Waiting;
                }

                let update = &tick.state.field_update;
                if update.attempt <= self.field_update_baseline {
                    if tick.now_ms.saturating_sub(self.step_started_ms) > timeout_s * 1000 {
                        return StepResult::Failed(Verdict::Error {
                            detail: "bootloader upload did not start before its timeout".into(),
                            advice: Some(
                                "check the selected firmware, ST-Link, and RS485 link".into(),
                            ),
                        });
                    }
                    return StepResult::Waiting;
                }

                match update.phase {
                    crate::state::FieldUpdatePhase::Passed => {
                        self.measurements.extend([
                            Measurement {
                                name: "field_update.application_bytes".into(),
                                value: MeasureValue::Number(update.application_bytes as f64),
                                unit: Some("bytes".into()),
                                at_ms: tick.now_ms.saturating_sub(self.started_ms),
                                step_index: self.index,
                            },
                            Measurement {
                                name: "field_update.expected_sha256".into(),
                                value: MeasureValue::Text(update.expected_sha256.clone()),
                                unit: None,
                                at_ms: tick.now_ms.saturating_sub(self.started_ms),
                                step_index: self.index,
                            },
                            Measurement {
                                name: "field_update.readback_sha256".into(),
                                value: MeasureValue::Text(update.readback_sha256.clone()),
                                unit: None,
                                at_ms: tick.now_ms.saturating_sub(self.started_ms),
                                step_index: self.index,
                            },
                            Measurement {
                                name: "field_update.bootloader_unchanged".into(),
                                value: MeasureValue::Bool(update.bootloader_unchanged),
                                unit: None,
                                at_ms: tick.now_ms.saturating_sub(self.started_ms),
                                step_index: self.index,
                            },
                            Measurement {
                                name: "field_update.persistent_unchanged".into(),
                                value: MeasureValue::Bool(update.persistent_unchanged),
                                unit: None,
                                at_ms: tick.now_ms.saturating_sub(self.started_ms),
                                step_index: self.index,
                            },
                            Measurement {
                                name: "field_update.application_booted".into(),
                                value: MeasureValue::Bool(update.application_booted),
                                unit: None,
                                at_ms: tick.now_ms.saturating_sub(self.started_ms),
                                step_index: self.index,
                            },
                            Measurement {
                                name: "field_update.beyond_64k".into(),
                                value: MeasureValue::Bool(update.beyond_64k),
                                unit: None,
                                at_ms: tick.now_ms.saturating_sub(self.started_ms),
                                step_index: self.index,
                            },
                        ]);
                        StepResult::Advance
                    }
                    crate::state::FieldUpdatePhase::Failed => {
                        StepResult::Failed(Verdict::Error {
                            detail: update.detail.clone(),
                            advice: Some(
                                "the application bank may be incomplete; recover it over SWD before using the board"
                                    .into(),
                            ),
                        })
                    }
                    _ if tick.now_ms.saturating_sub(self.step_started_ms) > timeout_s * 1000 => {
                        StepResult::Failed(Verdict::Error {
                            detail: format!("bootloader upload exceeded {timeout_s} s"),
                            advice: Some(
                                "check the RS485 adapter and recover the application over SWD"
                                    .into(),
                            ),
                        })
                    }
                    _ => StepResult::Waiting,
                }
            }

            _ => {
                // Everything else is a single op. Queue it once, then advance: waiting for the
                // *result* is an explicit `await_log` step, so a plan says out loud what it is
                // waiting for and for how long.
                if !self.dispatched {
                    if let Some(op) = step.op() {
                        tick.outbox.push(op);
                    }
                    self.dispatched = true;
                }
                StepResult::Advance
            }
        }
    }

    /// Read a measurement out of the current state and this run's log lines.
    fn read(&self, source: &Source, tick: &Tick<'_>) -> MeasureValue {
        let lines = tick.log.since(self.log_start_seq);
        match source {
            Source::Elapsed => MeasureValue::Number(
                (tick.now_ms.saturating_sub(self.step_started_ms)) as f64 / 1000.0,
            ),

            Source::LogNumber { after, before } => lines
                .iter()
                .rev()
                .find_map(|line| {
                    let rest = line.message.split_once(after.as_str())?.1;
                    let text = match before.is_empty() {
                        true => rest,
                        false => rest.split_once(before.as_str())?.0,
                    };
                    text.trim().parse::<f64>().ok()
                })
                .map_or(MeasureValue::Missing, MeasureValue::Number),

            Source::LogCount { min_level } => {
                MeasureValue::Number(lines.iter().filter(|l| l.level >= *min_level).count() as f64)
            }

            Source::LogSeen { contains } => {
                MeasureValue::Bool(lines.iter().any(|l| l.message.contains(contains.as_str())))
            }

            Source::Health { axis, flag } => match tick.state.dut.axis(*axis).health {
                Some(health) => MeasureValue::Bool(match flag {
                    HealthFlag::Home => health.home_ok,
                    HealthFlag::Switches => health.switches_ok,
                    HealthFlag::Backlash => health.backlash_ok,
                    HealthFlag::MeasureCycle => health.measure_cycle_ok,
                }),
                // The firmware never reported it. Not false -- unknown.
                None => MeasureValue::Missing,
            },

            Source::Position { axis } => tick
                .state
                .dut
                .axis(*axis)
                .position
                .map_or(MeasureValue::Missing, |p| MeasureValue::Number(p as f64)),

            Source::ThresholdApplied => {
                tick.state.dut.threshold.map_or(MeasureValue::Missing, |t| {
                    MeasureValue::Number(t.applied as f64)
                })
            }

            Source::ThresholdBand => tick.state.dut.threshold.map_or(MeasureValue::Missing, |t| {
                MeasureValue::Number(t.band as f64)
            }),

            Source::TransportDelta { counter } => {
                let current = match tick.state.active_channel {
                    Channel::Serial => &tick.state.channels.serial.diagnostics,
                    Channel::Rs485 => &tick.state.channels.rs485.diagnostics,
                };
                let (current, baseline) = match counter {
                    TransportCounter::Tx => (current.tx, self.transport_baseline.tx),
                    TransportCounter::Rx => (current.rx, self.transport_baseline.rx),
                    TransportCounter::Acks => (current.acks, self.transport_baseline.acks),
                    TransportCounter::AckTimeouts => {
                        (current.ack_timeouts, self.transport_baseline.ack_timeouts)
                    }
                    TransportCounter::DecodeErrors => {
                        (current.decode_errors, self.transport_baseline.decode_errors)
                    }
                };
                MeasureValue::Number(current.saturating_sub(baseline) as f64)
            }
        }
    }
}

fn unit_for(source: &Source) -> Option<String> {
    match source {
        Source::Elapsed => Some("s".into()),
        Source::Position { .. } => Some("usteps".into()),
        Source::ThresholdApplied | Source::ThresholdBand => Some("counts".into()),
        Source::TransportDelta { .. } => Some("packets".into()),
        _ => None,
    }
}

enum StepResult {
    Waiting,
    Advance,
    Failed(Verdict),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Body, Plan};
    use crate::verdict::{Criterion, Predicate};

    struct Harness {
        state: BenchState,
        log: LogRing,
        sent: Vec<Op>,
    }

    impl Harness {
        fn connected() -> Self {
            let mut state = BenchState::default();
            state.link.connected = true;
            Self {
                state,
                log: LogRing::default(),
                sent: Vec::new(),
            }
        }

        /// Run the engine to completion, with a clock the test controls entirely.
        fn run(&mut self, engine: &mut Engine, step_ms: u64, max_ticks: usize) -> Verdict {
            let mut now = 0u64;
            for _ in 0..max_ticks {
                let mut outbox = Vec::new();
                let mut tick = Tick {
                    now_ms: now,
                    state: &self.state,
                    log: &self.log,
                    outbox: &mut outbox,
                };
                let progress = engine.tick(&mut tick);
                self.sent.append(&mut outbox);
                if let Progress::Done(verdict) = progress {
                    return verdict;
                }
                now += step_ms;
            }
            panic!("engine did not finish within {max_ticks} ticks");
        }
    }

    fn plan(steps: Vec<Step>, criteria: Vec<Criterion>) -> Plan {
        Plan {
            name: "t".into(),
            body: Body::Once(steps),
            criteria,
            ..Plan::default()
        }
    }

    #[test]
    fn a_plan_whose_criteria_hold_passes() {
        let mut harness = Harness::connected();
        harness
            .log
            .push(0, 0, "firmware", "| END Routines.init".into());

        let mut engine = Engine::new(
            plan(
                vec![
                    Step::Connect,
                    Step::Record {
                        name: "finished".into(),
                        from: Source::LogSeen {
                            contains: "END Routines.init".into(),
                        },
                    },
                ],
                vec![Criterion {
                    measurement: "finished".into(),
                    must: Predicate::IsTrue,
                }],
            ),
            0,
            0,
        );
        assert_eq!(harness.run(&mut engine, 10, 50), Verdict::Pass);
    }

    /// The shape the startup plan uses: send the routine, then wait for the line the firmware
    /// prints only when it succeeded.
    #[test]
    fn await_log_completes_when_the_line_arrives() {
        let mut harness = Harness::connected();
        let mut engine = Engine::new(
            plan(
                vec![
                    Step::Startup,
                    Step::AwaitLog {
                        contains: "END Routines.init".into(),
                        timeout_s: 60,
                        fail_if: None,
                    },
                ],
                vec![],
            ),
            0,
            0,
        );

        // Two ticks in, the firmware answers.
        let mut now = 0;
        for tick_index in 0..10 {
            if tick_index == 3 {
                harness
                    .log
                    .push(now, 0, "firmware", "| END Routines.init".into());
            }
            let mut outbox = Vec::new();
            let mut tick = Tick {
                now_ms: now,
                state: &harness.state,
                log: &harness.log,
                outbox: &mut outbox,
            };
            let progress = engine.tick(&mut tick);
            harness.sent.append(&mut outbox);
            if let Progress::Done(verdict) = progress {
                assert_eq!(verdict, Verdict::Pass);
                assert!(harness.sent.contains(&Op::Startup));
                return;
            }
            now += 100;
        }
        panic!("the engine never finished");
    }

    /// The firmware announces its failures. Sitting out a four-minute timeout after it has
    /// already said no wastes the operator's time and buries the reason.
    #[test]
    fn await_log_fails_early_when_the_failure_line_arrives_first() {
        let mut harness = Harness::connected();
        harness
            .log
            .push(0, 20, "firmware", "[E Routines.init] Fail".into());

        let mut engine = Engine::new(
            plan(
                vec![Step::AwaitLog {
                    contains: "END Routines.init".into(),
                    timeout_s: 240,
                    fail_if: Some("[E ".into()),
                }],
                vec![],
            ),
            0,
            0,
        );
        // Far fewer ticks than the 240 s timeout would need.
        let verdict = harness.run(&mut engine, 100, 20);
        assert!(matches!(&verdict, Verdict::Fail { got, .. } if got.contains("Fail")));
    }

    #[test]
    fn await_log_times_out_rather_than_hanging() {
        let mut harness = Harness::connected();
        let mut engine = Engine::new(
            plan(
                vec![Step::AwaitLog {
                    contains: "never".into(),
                    timeout_s: 1,
                    fail_if: None,
                }],
                vec![],
            ),
            0,
            0,
        );
        let verdict = harness.run(&mut engine, 200, 50);
        assert!(matches!(&verdict, Verdict::Fail { got, .. } if got.contains("nothing matched")));
    }

    /// Teardown is where the escape and the restore live, so it has to run even when the plan
    /// is already failing.
    #[test]
    fn teardown_runs_even_when_the_body_failed() {
        let mut harness = Harness::connected();
        let mut engine = Engine::new(
            Plan {
                body: Body::Once(vec![Step::AwaitLog {
                    contains: "never".into(),
                    timeout_s: 1,
                    fail_if: None,
                }]),
                teardown: vec![Step::SetCurrent { amps: 0.15 }],
                ..Plan::default()
            },
            0,
            0,
        );
        let verdict = harness.run(&mut engine, 200, 50);
        assert!(matches!(verdict, Verdict::Fail { .. }));
        assert!(
            harness.sent.contains(&Op::SetCurrent { amps: 0.15 }),
            "teardown did not run: {:?}",
            harness.sent
        );
    }

    /// Abort must escape the routine and still run teardown, then report Aborted -- not Fail,
    /// because nothing was judged.
    #[test]
    fn cancelling_escapes_runs_teardown_and_reports_aborted() {
        let mut harness = Harness::connected();
        let mut engine = Engine::new(
            Plan {
                body: Body::Once(vec![Step::AwaitLog {
                    contains: "never".into(),
                    timeout_s: 600,
                    fail_if: None,
                }]),
                teardown: vec![Step::SetCurrent { amps: 0.15 }],
                ..Plan::default()
            },
            0,
            0,
        );

        let mut now = 0;
        let mut verdict = None;
        for tick_index in 0..40 {
            if tick_index == 2 {
                engine.cancel();
            }
            let mut outbox = Vec::new();
            let mut tick = Tick {
                now_ms: now,
                state: &harness.state,
                log: &harness.log,
                outbox: &mut outbox,
            };
            let progress = engine.tick(&mut tick);
            harness.sent.append(&mut outbox);
            if let Progress::Done(v) = progress {
                verdict = Some(v);
                break;
            }
            now += 100;
        }

        assert!(
            matches!(verdict, Some(Verdict::Aborted { .. })),
            "got {verdict:?}"
        );
        assert!(
            harness.sent.contains(&Op::Escape),
            "no escape was sent: {:?}",
            harness.sent
        );
        assert!(
            harness.sent.contains(&Op::SetCurrent { amps: 0.15 }),
            "teardown did not run"
        );
    }

    #[test]
    fn a_soak_repeats_its_body_the_requested_number_of_times() {
        let mut harness = Harness::connected();
        let mut engine = Engine::new(
            Plan {
                body: Body::Repeat {
                    until: Until::Cycles(3),
                    steps: vec![Step::Poll],
                },
                ..Plan::default()
            },
            0,
            0,
        );
        let verdict = harness.run(&mut engine, 10, 200);
        assert_eq!(verdict, Verdict::Pass);
        assert_eq!(harness.sent.iter().filter(|op| **op == Op::Poll).count(), 3);
        assert_eq!(engine.cycle(), 3);
    }

    #[test]
    fn the_overall_limit_stops_a_plan_that_would_otherwise_wait_forever() {
        let mut harness = Harness::connected();
        let mut engine = Engine::new(
            Plan {
                body: Body::Once(vec![Step::AwaitLog {
                    contains: "never".into(),
                    timeout_s: 10_000,
                    fail_if: None,
                }]),
                limits: crate::plan::Limits {
                    max_duration_s: 1,
                    ..Default::default()
                },
                ..Plan::default()
            },
            0,
            0,
        );
        let verdict = harness.run(&mut engine, 200, 100);
        assert!(matches!(&verdict, Verdict::Aborted { reason } if reason.contains("limit")));
    }

    /// `Duration: 42s` is the only place some routines say how long they took.
    #[test]
    fn a_number_can_be_lifted_out_of_a_firmware_log_line() {
        let mut harness = Harness::connected();
        harness
            .log
            .push(0, 0, "firmware", "[Routines.init] Duration: 42s".into());

        let mut engine = Engine::new(
            plan(
                vec![Step::Record {
                    name: "duration_s".into(),
                    from: Source::LogNumber {
                        after: "Duration: ".into(),
                        before: "s".into(),
                    },
                }],
                vec![Criterion {
                    measurement: "duration_s".into(),
                    must: Predicate::AtMost(240.0),
                }],
            ),
            0,
            0,
        );
        assert_eq!(harness.run(&mut engine, 10, 50), Verdict::Pass);
        assert_eq!(engine.measurements()[0].value, MeasureValue::Number(42.0));
    }

    /// A health flag the firmware never reported must record as missing, and a criterion over
    /// it must therefore fail rather than quietly pass.
    #[test]
    fn an_unreported_health_flag_records_as_missing_and_fails_its_criterion() {
        let mut harness = Harness::connected();
        let mut engine = Engine::new(
            plan(
                vec![Step::Record {
                    name: "home_ok".into(),
                    from: Source::Health {
                        axis: crate::dut::Axis::A,
                        flag: HealthFlag::Home,
                    },
                }],
                vec![Criterion {
                    measurement: "home_ok".into(),
                    must: Predicate::IsTrue,
                }],
            ),
            0,
            0,
        );
        let verdict = harness.run(&mut engine, 10, 50);
        assert!(matches!(verdict, Verdict::Fail { .. }));
        assert_eq!(engine.measurements()[0].value, MeasureValue::Missing);
    }

    #[test]
    fn connect_fails_with_advice_when_there_is_no_link() {
        let mut harness = Harness::connected();
        harness.state.link.connected = false;
        let mut engine = Engine::new(plan(vec![Step::Connect], vec![]), 0, 0);
        let verdict = harness.run(&mut engine, 1000, 50);
        assert!(
            matches!(&verdict, Verdict::Error { advice: Some(a), .. } if a.contains("connect"))
        );
    }

    #[test]
    fn bootloader_upload_waits_for_host_evidence_and_records_it() {
        let mut harness = Harness::connected();
        let mut engine = Engine::new(
            plan(vec![Step::BootloaderUpload { timeout_s: 90 }], vec![]),
            0,
            0,
        );

        let mut outbox = Vec::new();
        let mut tick = Tick {
            now_ms: 0,
            state: &harness.state,
            log: &harness.log,
            outbox: &mut outbox,
        };
        assert_eq!(engine.tick(&mut tick), Progress::Running);
        assert!(engine.take_effects().is_empty());
        let mut outbox = Vec::new();
        let mut tick = Tick {
            now_ms: 10,
            state: &harness.state,
            log: &harness.log,
            outbox: &mut outbox,
        };
        assert_eq!(engine.tick(&mut tick), Progress::Running);
        assert_eq!(
            engine.take_effects(),
            vec![Effect::BeginBootloaderUpload { timeout_s: 90 }]
        );

        harness.state.field_update.attempt = 1;
        harness.state.field_update.phase = crate::state::FieldUpdatePhase::Passed;
        harness.state.field_update.application_bytes = 95_048;
        harness.state.field_update.expected_sha256 = "expected".into();
        harness.state.field_update.readback_sha256 = "expected".into();
        harness.state.field_update.bootloader_unchanged = true;
        harness.state.field_update.persistent_unchanged = true;
        harness.state.field_update.application_booted = true;
        harness.state.field_update.beyond_64k = true;

        let verdict = harness.run(&mut engine, 10, 20);
        assert_eq!(verdict, Verdict::Pass);
        assert!(engine.measurements().iter().any(|measurement| {
            measurement.name == "field_update.application_bytes"
                && measurement.value == MeasureValue::Number(95_048.0)
        }));
        assert!(engine.measurements().iter().any(|measurement| {
            measurement.name == "field_update.application_booted"
                && measurement.value == MeasureValue::Bool(true)
        }));
    }
}
