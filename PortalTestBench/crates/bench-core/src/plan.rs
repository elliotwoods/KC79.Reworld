//! What a test *is*.
//!
//! A plan is a Rust value that happens to have a TOML spelling — `Step` derives `Deserialize`,
//! so a plan file is the type, not a script a separate interpreter walks. There is one
//! executor ([`crate::engine`]) and two front doors: the GUI builds a `Vec<Step>` from its
//! buttons, and a plan file deserialises into the same thing.
//!
//! # Validation happens before anything moves
//!
//! [`Plan::validate`] is the guard rail. It refuses a plan whose transport cannot express one
//! of its steps, whose firmware requirement does not match the module, or — most importantly —
//! that homes an optical module without first calibrating the threshold. Catching those at the
//! start is the difference between "this plan is wrong" and "the motor moved for ninety seconds
//! and then the plan turned out to be wrong".

use serde::{Deserialize, Serialize};

use crate::dut::{Axis, FirmwareKind, GearRatio};
use crate::transport::{LinkKind, MotionProfile, Op};
use crate::verdict::Criterion;

/// What a plan needs before it can run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Requires {
    /// `None` means "either"; a plan that genuinely works both ways.
    pub firmware: Option<FirmwareKind>,
    /// Restrict to one transport. Rarely needed — `validate` already refuses steps a transport
    /// cannot express, which is the check that actually matters.
    pub transport: Option<TransportRequirement>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TransportRequirement {
    Vcp,
    Rs485,
    BenchAscii,
}

impl TransportRequirement {
    pub fn accepts(self, kind: LinkKind) -> bool {
        match self {
            Self::Vcp => kind == LinkKind::Vcp,
            Self::Rs485 => matches!(kind, LinkKind::Rs485Serial | LinkKind::Rs485Tcp),
            Self::BenchAscii => kind == LinkKind::BenchAscii,
        }
    }
}

/// Bounds the engine enforces above whatever the individual steps do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub max_duration_s: u64,
    /// Above this, a move is refused outside a `characterise` plan.
    ///
    /// The 32:1 stall cliff sits at 30–32k µsteps/s when ramped at 100k/s². 28k leaves margin
    /// on a cold motor without being so low that ordinary repositioning is slow.
    pub stall_guard_usteps_per_s: i32,
    /// Stop a soak after this many consecutive failing cycles.
    pub stop_after_consecutive_fails: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_duration_s: 600,
            stall_guard_usteps_per_s: 28_000,
            stop_after_consecutive_fails: 3,
        }
    }
}

/// What kind of test this is. Decides which safety rails apply.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Run routines and judge the result.
    #[default]
    Routine,
    /// Repeat until a count, a duration or a failure.
    Soak,
    /// Deliberately explore the envelope. **The stall guard does not apply** — finding the
    /// cliff is the entire job.
    Characterise,
    /// Exercise the link rather than the mechanism.
    BusDiag,
}

/// Where a recorded measurement comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "from", rename_all = "snake_case")]
pub enum Source {
    /// Seconds spent on the preceding step.
    Elapsed,
    /// A number lifted out of a log line: the text after `after`, up to `before`.
    ///
    /// Used for the firmware's own `Duration: 42s` lines, which are the only place some
    /// routines report how long they took.
    LogNumber {
        after: String,
        before: String,
    },
    /// How many log lines at or above `min_level` have arrived since the plan started.
    LogCount {
        min_level: u8,
    },
    /// Whether a log line containing this text was seen.
    LogSeen {
        contains: String,
    },
    /// One of an axis's four health flags.
    Health {
        axis: Axis,
        flag: HealthFlag,
    },
    Position {
        axis: Axis,
    },
    /// The applied optical threshold, and the band it came from.
    ThresholdApplied,
    ThresholdBand,
    TransportDelta {
        counter: TransportCounter,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportCounter {
    Tx,
    Rx,
    Acks,
    AckTimeouts,
    DecodeErrors,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthFlag {
    Home,
    Switches,
    Backlash,
    MeasureCycle,
}

/// One thing the engine does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Step {
    Connect,
    Identify,
    Poll,
    /// The firmware's whole-module startup routine: measure cycle, then calibrate, then both
    /// axes to zero. Minutes, not seconds.
    Startup,
    Home {
        axis: Axis,
    },
    Calibrate {
        axis: Axis,
    },
    Unjam {
        axis: Axis,
    },
    MeasureBacklash {
        axis: Axis,
    },
    MoveTo {
        axis: Axis,
        usteps: i32,
        #[serde(default)]
        profile: Option<MotionProfile>,
    },
    SetCurrent {
        amps: f32,
    },
    SetMicrostep {
        resolution: u32,
    },
    SetHomeThreshold {
        value: i32,
    },
    /// Measure the optical band and adopt its operating point.
    CalibrateThreshold {
        axis: Axis,
        #[serde(default = "one")]
        passes: u8,
    },
    /// Wait for a log line containing `contains`, or fail after `timeout_s`.
    ///
    /// This is how a routine's *completion* is detected. The firmware acknowledges a routine
    /// immediately and then runs for seconds to minutes, so there is nothing else to wait on.
    AwaitLog {
        contains: String,
        timeout_s: u64,
        /// If this text is seen first, the step fails immediately rather than waiting out the
        /// timeout. The firmware logs its failures; there is no reason to sit for four minutes
        /// after it has already said no.
        #[serde(default)]
        fail_if: Option<String>,
    },
    Sleep {
        ms: u64,
    },
    Record {
        name: String,
        #[serde(flatten)]
        from: Source,
    },
    Marker {
        label: String,
    },
    Escape,
    /// Re-upload the selected application through the resident RS485 bootloader, then verify it
    /// independently over SWD. The host adapter supplies the selected bytes and probe evidence.
    BootloaderUpload {
        #[serde(default = "default_field_update_timeout_s")]
        timeout_s: u64,
    },
}

fn one() -> u8 {
    1
}

fn default_field_update_timeout_s() -> u64 {
    90
}

impl Step {
    /// A short human name for the progress display.
    ///
    /// Worth naming precisely: `/run/step_name` is what turns a minute-long silence into
    /// "Startup routine — 34 s of 240 s" instead of a spinner.
    pub fn name(&self) -> String {
        match self {
            Step::Connect => "Connect".into(),
            Step::Identify => "Identify".into(),
            Step::Poll => "Poll".into(),
            Step::Startup => "Startup routine".into(),
            Step::Home { axis } => format!("Home {}", axis.suffix()),
            Step::Calibrate { axis } => format!("Calibrate {}", axis.suffix()),
            Step::Unjam { axis } => format!("Unjam {}", axis.suffix()),
            Step::MeasureBacklash { axis } => format!("Measure backlash {}", axis.suffix()),
            Step::MoveTo { axis, usteps, .. } => format!("Move {} to {usteps}", axis.suffix()),
            Step::SetCurrent { amps } => format!("Set current {amps} A"),
            Step::SetMicrostep { resolution } => format!("Set microstep 1/{resolution}"),
            Step::SetHomeThreshold { value } => format!("Set threshold {value}"),
            Step::CalibrateThreshold { axis, .. } => {
                format!("Calibrate threshold {}", axis.suffix())
            }
            Step::AwaitLog { contains, .. } => format!("Wait for \"{contains}\""),
            Step::Sleep { ms } => format!("Wait {ms} ms"),
            Step::Record { name, .. } => format!("Record {name}"),
            Step::Marker { label } => format!("Marker: {label}"),
            Step::Escape => "Escape routine".into(),
            Step::BootloaderUpload { .. } => "Bootloader upload check".into(),
        }
    }

    /// The op this step puts on the wire, if any. Steps that only observe return `None`.
    pub fn op(&self) -> Option<Op> {
        Some(match self {
            Step::Identify => Op::Identify,
            Step::Poll => Op::Poll,
            Step::Startup => Op::Startup,
            Step::Home { axis } => Op::Home { axis: *axis },
            Step::Calibrate { axis } => Op::Calibrate { axis: *axis },
            Step::Unjam { axis } => Op::Unjam { axis: *axis },
            Step::MeasureBacklash { axis } => Op::MeasureBacklash { axis: *axis },
            Step::MoveTo {
                axis,
                usteps,
                profile,
            } => Op::MoveTo {
                axis: *axis,
                usteps: *usteps,
                profile: *profile,
            },
            Step::SetCurrent { amps } => Op::SetCurrent { amps: *amps },
            Step::SetMicrostep { resolution } => Op::SetMicrostep {
                resolution: *resolution,
            },
            Step::SetHomeThreshold { value } => Op::SetHomeThreshold { value: *value },
            Step::Escape => Op::Escape,
            Step::Connect
            | Step::CalibrateThreshold { .. }
            | Step::AwaitLog { .. }
            | Step::Sleep { .. }
            | Step::Record { .. }
            | Step::Marker { .. }
            | Step::BootloaderUpload { .. } => return None,
        })
    }
}

/// How many times the body runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Body {
    Once(Vec<Step>),
    Repeat {
        #[serde(flatten)]
        until: Until,
        steps: Vec<Step>,
    },
}

impl Body {
    pub fn steps(&self) -> &[Step] {
        match self {
            Body::Once(steps) => steps,
            Body::Repeat { steps, .. } => steps,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Until {
    Cycles(u32),
    DurationS(u64),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Plan {
    pub name: String,
    pub kind: Kind,
    pub requires: Requires,
    pub setup: Vec<Step>,
    pub body: Body,
    /// Always runs, on every exit path including an abort or an error.
    pub teardown: Vec<Step>,
    pub criteria: Vec<Criterion>,
    pub limits: Limits,
}

impl Default for Plan {
    fn default() -> Self {
        Self {
            name: "unnamed".into(),
            kind: Kind::default(),
            requires: Requires::default(),
            setup: Vec::new(),
            body: Body::Once(Vec::new()),
            teardown: Vec::new(),
            criteria: Vec::new(),
            limits: Limits::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error("plan has no steps to run")]
    Empty,

    #[error("plan needs {needed:?} firmware, but this module is running {found:?}")]
    WrongFirmware {
        needed: FirmwareKind,
        found: FirmwareKind,
    },

    #[error("plan needs {needed:?}, but the selected link is {found}")]
    WrongTransport {
        needed: TransportRequirement,
        found: &'static str,
    },

    #[error("the {transport} link cannot carry step {index} ({step})")]
    UnsupportedStep {
        transport: &'static str,
        index: usize,
        step: String,
    },

    #[error(
        "step {index} ({step}) homes an optical module, but nothing calibrated the threshold \
         first. The production ring's background is unmeasurable, so the operating point can \
         only come from a per-run census -- add a `calibrate_threshold` step, or set \
         `threshold_fixed` with a reason."
    )]
    UncalibratedHome { index: usize, step: String },

    #[error(
        "step {index} moves at {speed} usteps/s, past the {limit} stall guard. Only a \
         `characterise` plan may cross it -- that is what such a plan is for."
    )]
    PastStallGuard {
        index: usize,
        speed: i32,
        limit: i32,
    },

    #[error("criterion refers to measurement `{0}`, which no step records")]
    DanglingCriterion(String),

    #[error("bootloader_upload must appear exactly once in a non-repeating plan body")]
    UnsafeFieldUpdatePlacement,

    #[error("bootloader_upload requires an RS485 transport")]
    FieldUpdateNeedsRs485,
}

/// What the module looks like right now, for validation purposes.
#[derive(Debug, Clone, Copy)]
pub struct ValidationContext {
    pub transport: LinkKind,
    pub firmware: FirmwareKind,
    pub ratio: GearRatio,
    /// True when the module uses the optical home switch, so a home needs a live threshold.
    pub optical: bool,
    /// True when a threshold has already been established this session.
    pub threshold_calibrated: bool,
}

impl Plan {
    /// Every step, in execution order, with its index.
    pub fn all_steps(&self) -> Vec<&Step> {
        self.setup
            .iter()
            .chain(self.body.steps())
            .chain(self.teardown.iter())
            .collect()
    }

    /// Refuse a plan that cannot work, before anything moves.
    pub fn validate(&self, context: &ValidationContext) -> Result<(), PlanError> {
        if self.all_steps().is_empty() {
            return Err(PlanError::Empty);
        }

        if let Some(needed) = self.requires.firmware
            && context.firmware != needed
        {
            return Err(PlanError::WrongFirmware {
                needed,
                found: context.firmware,
            });
        }

        if let Some(needed) = self.requires.transport
            && !needed.accepts(context.transport)
        {
            return Err(PlanError::WrongTransport {
                needed,
                found: context.transport.name(),
            });
        }

        let field_updates = self
            .all_steps()
            .into_iter()
            .filter(|step| matches!(step, Step::BootloaderUpload { .. }))
            .count();
        if field_updates > 0 {
            let in_once_body = matches!(
                &self.body,
                Body::Once(steps)
                    if steps
                        .iter()
                        .filter(|step| matches!(step, Step::BootloaderUpload { .. }))
                        .count()
                        == 1
            ) && !self
                .setup
                .iter()
                .any(|step| matches!(step, Step::BootloaderUpload { .. }))
                && !self
                    .teardown
                    .iter()
                    .any(|step| matches!(step, Step::BootloaderUpload { .. }));
            if field_updates != 1 || !in_once_body {
                return Err(PlanError::UnsafeFieldUpdatePlacement);
            }
            if !matches!(
                context.transport,
                LinkKind::Rs485Serial | LinkKind::Rs485Tcp
            ) {
                return Err(PlanError::FieldUpdateNeedsRs485);
            }
        }

        // Track whether a threshold is live as we walk the plan: a `calibrate_threshold` step
        // earlier in the same plan satisfies a later home, which is the normal shape.
        let mut threshold_live = context.threshold_calibrated;

        for (index, step) in self.all_steps().into_iter().enumerate() {
            if let Some(op) = step.op()
                && !transport_can(context.transport, &op)
            {
                return Err(PlanError::UnsupportedStep {
                    transport: context.transport.name(),
                    index,
                    step: step.name(),
                });
            }

            match step {
                // These four establish an operating threshold rather than needing one.
                //
                // `startup` and `calibrate` run the firmware's own self-calibrating optical
                // home (the seven-phase `fastHomeRoutine`: guard, seek, detect, band, operate,
                // precise, backlash), which measures the band on the module and caches the
                // operating point. That is exactly the per-run census the rule demands, done in
                // the right place -- by the moving comparator rather than by the host.
                Step::Startup
                | Step::Calibrate { .. }
                | Step::CalibrateThreshold { .. }
                | Step::SetHomeThreshold { .. } => {
                    threshold_live = true;
                }
                // A bare `home`, by contrast, uses whatever threshold is already cached. On a
                // module that has not calibrated this session that is a stale constant, which
                // is the failure this rule exists to prevent.
                Step::Home { .. } if context.optical && !threshold_live => {
                    return Err(PlanError::UncalibratedHome {
                        index,
                        step: step.name(),
                    });
                }
                Step::MoveTo {
                    profile: Some(profile),
                    ..
                } if self.kind != Kind::Characterise
                    && profile.max_velocity > self.limits.stall_guard_usteps_per_s =>
                {
                    return Err(PlanError::PastStallGuard {
                        index,
                        speed: profile.max_velocity,
                        limit: self.limits.stall_guard_usteps_per_s,
                    });
                }
                _ => {}
            }
        }

        // A criterion naming a measurement nothing records can never be satisfied, and a plan
        // whose verdict is structurally unreachable should fail now rather than at the end.
        let recorded: Vec<&str> = self
            .all_steps()
            .into_iter()
            .filter_map(|step| match step {
                Step::Record { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        for criterion in &self.criteria {
            if !recorded.contains(&criterion.measurement.as_str()) {
                return Err(PlanError::DanglingCriterion(criterion.measurement.clone()));
            }
        }

        Ok(())
    }

    pub fn is_destructive(&self) -> bool {
        self.all_steps()
            .iter()
            .any(|step| matches!(step, Step::BootloaderUpload { .. }))
    }
}

/// Whether a transport can express an op, without needing an open link to ask.
fn transport_can(kind: LinkKind, op: &Op) -> bool {
    match kind {
        LinkKind::BenchAscii => crate::transport::ascii::render_op(op).is_some(),
        LinkKind::Vcp => crate::transport::line::vcp_can(op),
        // The RS485 dialect covers the whole vocabulary.
        LinkKind::Rs485Serial | LinkKind::Rs485Tcp | LinkKind::Sim => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verdict::{Criterion, Predicate};

    fn context() -> ValidationContext {
        ValidationContext {
            transport: LinkKind::Rs485Serial,
            firmware: FirmwareKind::Production,
            ratio: GearRatio::R32,
            optical: true,
            threshold_calibrated: false,
        }
    }

    fn plan_with(steps: Vec<Step>) -> Plan {
        Plan {
            name: "t".into(),
            body: Body::Once(steps),
            ..Plan::default()
        }
    }

    /// The invariant that matters most in this whole crate. The production ring's background
    /// never crosses at any threshold, so `T = background - k` is impossible; the operating
    /// point can only come from a census of the flag itself. A bare home uses whatever is
    /// cached, so a plan that homes without establishing one is refused *before* the motor
    /// turns.
    #[test]
    fn homing_an_optical_module_without_establishing_a_threshold_is_refused() {
        let plan = plan_with(vec![Step::Home { axis: Axis::A }]);
        let error = plan.validate(&context()).unwrap_err();
        assert!(
            matches!(error, PlanError::UncalibratedHome { .. }),
            "got {error:?}"
        );
    }

    /// `startup` and `calibrate` run the firmware's own self-calibrating optical home, which
    /// measures the band on the module. They *establish* the threshold, so requiring one first
    /// would be backwards -- and would make the one routine that does the right thing
    /// impossible to run on a cold module.
    #[test]
    fn the_self_calibrating_routines_establish_a_threshold_rather_than_needing_one() {
        assert!(plan_with(vec![Step::Startup]).validate(&context()).is_ok());
        assert!(
            plan_with(vec![Step::Calibrate { axis: Axis::A }])
                .validate(&context())
                .is_ok()
        );

        // And having run one, a later bare home is fine.
        let plan = plan_with(vec![
            Step::Calibrate { axis: Axis::A },
            Step::Home { axis: Axis::A },
        ]);
        assert!(plan.validate(&context()).is_ok());
    }

    #[test]
    fn a_calibrate_step_earlier_in_the_same_plan_satisfies_a_later_home() {
        let plan = plan_with(vec![
            Step::CalibrateThreshold {
                axis: Axis::A,
                passes: 3,
            },
            Step::Home { axis: Axis::A },
        ]);
        assert!(plan.validate(&context()).is_ok());
    }

    #[test]
    fn a_threshold_already_measured_this_session_satisfies_a_home() {
        let plan = plan_with(vec![Step::Home { axis: Axis::A }]);
        let context = ValidationContext {
            threshold_calibrated: true,
            ..context()
        };
        assert!(plan.validate(&context).is_ok());
    }

    /// A mechanical-switch module has no optical threshold to calibrate, so the rule must not
    /// fire and block it.
    #[test]
    fn a_mechanical_module_may_home_without_a_threshold() {
        let plan = plan_with(vec![Step::Home { axis: Axis::A }]);
        let context = ValidationContext {
            optical: false,
            ..context()
        };
        assert!(plan.validate(&context).is_ok());
    }

    #[test]
    fn a_move_past_the_stall_guard_is_refused_outside_a_characterise_plan() {
        let fast = MotionProfile {
            max_velocity: 34_000,
            acceleration: 100_000,
            min_velocity: 1_000,
        };
        let mut plan = plan_with(vec![
            Step::CalibrateThreshold {
                axis: Axis::A,
                passes: 1,
            },
            Step::MoveTo {
                axis: Axis::A,
                usteps: 1000,
                profile: Some(fast),
            },
        ]);
        assert!(matches!(
            plan.validate(&context()).unwrap_err(),
            PlanError::PastStallGuard { .. }
        ));

        // Finding the cliff is precisely what a characterisation plan is for.
        plan.kind = Kind::Characterise;
        assert!(plan.validate(&context()).is_ok());
    }

    #[test]
    fn a_plan_needing_the_other_firmware_is_refused_by_name() {
        let plan = Plan {
            requires: Requires {
                firmware: Some(FirmwareKind::Bench),
                transport: None,
            },
            body: Body::Once(vec![Step::Poll]),
            ..Plan::default()
        };
        let error = plan.validate(&context()).unwrap_err();
        assert!(matches!(
            error,
            PlanError::WrongFirmware {
                needed: FirmwareKind::Bench,
                found: FirmwareKind::Production
            }
        ));
    }

    /// Operations without a production VCOM form are refused before a run begins.
    #[test]
    fn a_step_the_transport_cannot_express_is_refused_up_front() {
        let plan = plan_with(vec![Step::MeasureBacklash { axis: Axis::A }]);
        let context = ValidationContext {
            transport: LinkKind::Vcp,
            ..context()
        };
        let error = plan.validate(&context).unwrap_err();
        assert!(
            matches!(
                error,
                PlanError::UnsupportedStep {
                    transport: "vcp",
                    ..
                }
            ),
            "got {error:?}"
        );
    }

    /// A verdict that can never be reached is a plan defect, not a result.
    #[test]
    fn a_criterion_with_no_matching_record_step_is_refused() {
        let plan = Plan {
            body: Body::Once(vec![Step::Poll]),
            criteria: vec![Criterion {
                measurement: "backlash".into(),
                must: Predicate::AtMost(900.0),
            }],
            ..Plan::default()
        };
        assert!(matches!(
            plan.validate(&context()).unwrap_err(),
            PlanError::DanglingCriterion(name) if name == "backlash"
        ));
    }

    #[test]
    fn an_empty_plan_is_refused() {
        assert!(matches!(
            Plan::default().validate(&context()).unwrap_err(),
            PlanError::Empty
        ));
    }

    /// A plan file is the Rust type, spelled in TOML. If this drifts, plans in the repository
    /// stop loading.
    ///
    /// Note `requires` is written **inline**. A `[requires]` header would swallow every
    /// top-level key after it — TOML puts everything following a table header inside that
    /// table — so `setup` would be read as `requires.setup` and rejected. Plans in `plans/`
    /// use the inline form for exactly this reason.
    #[test]
    fn a_toml_plan_round_trips_into_the_rust_type() {
        let text = r#"
name = "startup"
kind = "routine"
requires = { firmware = "production" }

setup = [
  { op = "connect" },
  { op = "identify" },
]

[body]
once = [
  { op = "startup" },
  { op = "await_log", contains = "END Routines.init", timeout_s = 240, fail_if = "[E " },
  { op = "record", name = "duration_s", from = "log_number", after = "Duration: ", before = "s" },
]

[[criteria]]
measurement = "duration_s"
must = { at_most = 240.0 }
"#;
        let plan: Plan = toml::from_str(text).expect("plan should parse");
        assert_eq!(plan.name, "startup");
        assert_eq!(plan.requires.firmware, Some(FirmwareKind::Production));
        assert_eq!(plan.body.steps().len(), 3);
        assert_eq!(plan.criteria.len(), 1);
    }

    #[test]
    fn bootloader_upload_is_one_destructive_rs485_body_step() {
        let plan = Plan {
            requires: Requires {
                firmware: Some(FirmwareKind::Production),
                transport: Some(TransportRequirement::Rs485),
            },
            body: Body::Once(vec![Step::BootloaderUpload { timeout_s: 90 }]),
            ..Plan::default()
        };
        assert!(plan.validate(&context()).is_ok());
        assert!(plan.is_destructive());

        let mut vcp = context();
        vcp.transport = LinkKind::Vcp;
        assert!(matches!(
            plan.validate(&vcp),
            Err(PlanError::WrongTransport { .. } | PlanError::FieldUpdateNeedsRs485)
        ));

        let repeated = Plan {
            body: Body::Repeat {
                until: Until::Cycles(2),
                steps: vec![Step::BootloaderUpload { timeout_s: 90 }],
            },
            ..Plan::default()
        };
        assert_eq!(
            repeated.validate(&context()),
            Err(PlanError::UnsafeFieldUpdatePlacement)
        );
    }
}

/// Load a plan from a TOML file.
pub fn load(path: &std::path::Path) -> Result<Plan, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    toml::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))
}

/// Every `*.toml` in a directory, by name.
pub fn load_dir(dir: &std::path::Path) -> Vec<(String, Result<Plan, String>)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut plans: Vec<(String, Result<Plan, String>)> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "toml"))
        .map(|path| {
            let name = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            (name, load(&path))
        })
        .collect();
    plans.sort_by(|a, b| a.0.cmp(&b.0));
    plans
}

#[cfg(test)]
mod repository_tests {
    /// Every plan shipped in `plans/` must parse. A plan file that only fails when someone
    /// tries to run it is a trap laid for whoever is at the bench that day.
    #[test]
    fn every_plan_in_the_repository_parses() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plans");
        let plans = super::load_dir(&dir);
        assert!(!plans.is_empty(), "no plans found in {}", dir.display());
        for (name, result) in plans {
            assert!(
                result.is_ok(),
                "{name} did not parse: {}",
                result.unwrap_err()
            );
        }
    }
}
