//! A Portal: one dual-prism steering unit on the bus. Holds the Pilot, the
//! per-axis MotionControl/MotorDriver parameter blocks, the settings module,
//! the firmware log history, and the hardware-reported app state.
//! Port of `Router/src/Modules/Hardware/Portal.*` and `PerPortal/*`.

use std::time::Instant;

use router_proto::commands::{self, ActionKind, Axis, LocalEffect, MeasureSettings};
use router_proto::replies::{self, LogMessage, PortalReport, Reply};
use router_proto::Value;

use super::pilot::{AxisReported, Pilot};

// ------------------------------------------------------- MotionControl

/// Parameters + reported state of one axis's MotionControl
/// (`PerPortal/MotionControl.*`). FW keys "motionControlA"/"motionControlB".
#[derive(Debug, Clone)]
pub struct MotionControl {
    pub axis: Axis,
    // motionProfile parameters
    pub max_velocity: i32,
    pub acceleration: i32,
    pub min_velocity: i32,
    // measureSettings parameters
    pub measure: MeasureSettings,
    // reported state
    pub reported_position: Option<i32>,
    pub reported_target: Option<i32>,
    pub reported_health: replies::HealthStatus,
    // cached sent motion profile for auto-push on change
    sent_profile: Option<(i32, i32, i32)>,
}

impl MotionControl {
    pub fn new(axis: Axis) -> Self {
        Self {
            axis,
            max_velocity: 30_000,
            acceleration: 10_000,
            min_velocity: 100,
            measure: MeasureSettings::default(),
            reported_position: None,
            reported_target: None,
            reported_health: Default::default(),
            sent_profile: None,
        }
    }

    /// Auto-push of the motion profile when parameters changed vs. the
    /// cached sent values (`MotionControl::update`). Returns the message to
    /// send, if due.
    pub fn profile_push_due(&mut self) -> Option<Value> {
        let profile = (self.max_velocity, self.acceleration, self.min_velocity);
        if self.sent_profile == Some(profile) {
            return None;
        }
        // Only push automatically once something was sent or params changed
        // from defaults; C++ pushes whenever any param differs from cache.
        if self.sent_profile.is_none() {
            // Initialize the cache without pushing on startup (the C++ caches
            // are initialized from the parameters at construction).
            self.sent_profile = Some(profile);
            return None;
        }
        self.sent_profile = Some(profile);
        Some(commands::mc_motion_profile(
            self.axis,
            profile.0,
            profile.1,
            profile.2,
        ))
    }

    pub fn ingest(&mut self, status: &replies::MotionControlStatus) {
        if status.position.is_some() {
            self.reported_position = status.position;
        }
        if status.target_position.is_some() {
            self.reported_target = status.target_position;
        }
        let h = &status.health;
        if h.measure_cycle_ok.is_some() {
            self.reported_health.measure_cycle_ok = h.measure_cycle_ok;
        }
        if h.switches_ok.is_some() {
            self.reported_health.switches_ok = h.switches_ok;
        }
        if h.backlash_ok.is_some() {
            self.reported_health.backlash_ok = h.backlash_ok;
        }
        if h.home_ok.is_some() {
            self.reported_health.home_ok = h.home_ok;
        }
    }
}

// -------------------------------------------------------- MotorDriver

/// `PerPortal/MotorDriver.*` testTimer parameters.
#[derive(Debug, Clone)]
pub struct MotorDriver {
    pub axis: Axis,
    pub test_timer_count: u32,
    pub test_timer_period_us: u32,
    pub normalise_parameters: bool,
}

impl MotorDriver {
    pub fn new(axis: Axis) -> Self {
        Self {
            axis,
            test_timer_count: 4000,
            test_timer_period_us: 500,
            normalise_parameters: true,
        }
    }

    /// BUG-COMPAT: the C++ computes normalized period/count but packs the
    /// raw parameters regardless (`MotorDriver.cpp` testTimer). We keep the
    /// C++ wire behavior: raw values are sent.
    pub fn test_timer_message(&self) -> Value {
        commands::md_test_timer(self.axis, self.test_timer_period_us, self.test_timer_count)
    }
}

// ----------------------------------------------- MotorDriverSettings

#[derive(Debug, Clone)]
pub struct MotorDriverSettings {
    pub auto_push: bool,
    pub current_amps: f32,
    pub microstep_resolution: u32,
    sent_current: Option<f32>,
    sent_microstep: Option<u32>,
}

impl Default for MotorDriverSettings {
    fn default() -> Self {
        Self {
            auto_push: false,
            current_amps: 0.25,
            microstep_resolution: 32,
            sent_current: None,
            sent_microstep: None,
        }
    }
}

impl MotorDriverSettings {
    /// Auto-push messages due this frame (when `auto_push` is on).
    pub fn push_due(&mut self) -> Vec<Value> {
        let mut messages = Vec::new();
        if !self.auto_push {
            return messages;
        }
        if self.sent_current != Some(self.current_amps) {
            self.sent_current = Some(self.current_amps);
            messages.push(commands::mds_set_current(self.current_amps));
        }
        if self.sent_microstep != Some(self.microstep_resolution) {
            self.sent_microstep = Some(self.microstep_resolution);
            messages.push(commands::mds_set_microstep_resolution(self.microstep_resolution));
        }
        messages
    }

    pub fn mark_current_sent(&mut self) {
        self.sent_current = Some(self.current_amps);
    }

    pub fn mark_microstep_sent(&mut self) {
        self.sent_microstep = Some(self.microstep_resolution);
    }
}

// ------------------------------------------------------------- Logger

/// Firmware log history with consecutive-duplicate dedup
/// (`PerPortal/Logger.*`).
#[derive(Debug, Clone)]
pub struct StoredLogMessage {
    pub message: String,
    pub level: u8,
    pub timestamp_ms: Option<u64>,
    pub count: u32,
    pub received: Instant,
}

#[derive(Debug, Clone)]
pub struct PortalLogger {
    pub messages: Vec<StoredLogMessage>,
    pub max_history_size: usize,
}

impl Default for PortalLogger {
    fn default() -> Self {
        Self {
            messages: Vec::new(),
            max_history_size: 100,
        }
    }
}

impl PortalLogger {
    /// Returns the messages that were newly added (for reporting).
    pub fn ingest(&mut self, logs: &[LogMessage]) -> Vec<LogMessage> {
        let mut fresh = Vec::new();
        for log in logs {
            if let Some(last) = self.messages.last_mut() {
                if last.message == log.message && last.level == log.level {
                    last.count += 1;
                    last.timestamp_ms = log.timestamp_ms.or(last.timestamp_ms);
                    last.received = Instant::now();
                    fresh.push(log.clone());
                    continue;
                }
            }
            self.messages.push(StoredLogMessage {
                message: log.message.clone(),
                level: log.level,
                timestamp_ms: log.timestamp_ms,
                count: 1,
                received: Instant::now(),
            });
            fresh.push(log.clone());
        }
        // truncate oldest
        if self.messages.len() > self.max_history_size {
            let excess = self.messages.len() - self.max_history_size;
            self.messages.drain(0..excess);
        }
        fresh
    }

    pub fn last_message(&self) -> Option<&StoredLogMessage> {
        self.messages.last()
    }
}

// ------------------------------------------------------------- Portal

#[derive(Debug, Clone, Default)]
pub struct ReportedAppState {
    pub up_time_ms: Option<u64>,
    pub version: Option<String>,
    pub calibrated: Option<bool>,
}

pub struct Portal {
    pub target: u8,
    pub pilot: Pilot,
    pub motion_control: [MotionControl; 2],
    pub motor_driver: [MotorDriver; 2],
    pub motor_driver_settings: MotorDriverSettings,
    pub logger: PortalLogger,
    pub reported: ReportedAppState,
    // poll parameters
    pub poll_regularly: bool,
    pub poll_interval_s: f32,
    pub last_poll: Option<Instant>,
    // heartbeats
    pub last_rx: Option<Instant>,
    pub last_tx: Option<Instant>,
}

/// A message this portal wants sent, with its collation address.
pub struct OutgoingMessage {
    pub body: Value,
    pub address: String,
}

impl Portal {
    pub fn new(target: u8) -> Self {
        Self {
            target,
            pilot: Pilot::default(),
            motion_control: [MotionControl::new(Axis::A), MotionControl::new(Axis::B)],
            motor_driver: [MotorDriver::new(Axis::A), MotorDriver::new(Axis::B)],
            motor_driver_settings: MotorDriverSettings::default(),
            logger: PortalLogger::default(),
            reported: ReportedAppState::default(),
            poll_regularly: false,
            poll_interval_s: 1.0,
            last_poll: None,
            last_rx: None,
            last_tx: None,
        }
    }

    /// Per-frame update: pilot sync + auto-push messages due (motion profile
    /// changes, motor driver settings) + scheduled polls.
    pub fn update(&mut self) -> Vec<OutgoingMessage> {
        let reported = [
            AxisReported {
                current_steps: self.motion_control[0].reported_position,
                target_steps: self.motion_control[0].reported_target,
            },
            AxisReported {
                current_steps: self.motion_control[1].reported_position,
                target_steps: self.motion_control[1].reported_target,
            },
        ];
        self.pilot.update(reported);

        let mut outgoing = Vec::new();
        for mc in &mut self.motion_control {
            if let Some(body) = mc.profile_push_due() {
                outgoing.push(OutgoingMessage {
                    address: commands::mc_address(mc.axis, "motionProfile"),
                    body,
                });
            }
        }
        for body in self.motor_driver_settings.push_due() {
            outgoing.push(OutgoingMessage {
                address: "motorDriverSettings".into(),
                body,
            });
        }

        // regular polling
        if self.poll_regularly {
            let due = self
                .last_poll
                .map(|t| t.elapsed().as_secs_f32() >= self.poll_interval_s)
                .unwrap_or(true);
            if due {
                self.last_poll = Some(Instant::now());
                outgoing.push(OutgoingMessage {
                    address: "poll".into(),
                    body: commands::poll(),
                });
            }
        }

        outgoing
    }

    /// Route an incoming message body from this portal
    /// (`Portal::processIncoming`). Returns any freshly-received log
    /// messages for the reporter, plus whether a report was parsed.
    pub fn process_incoming(&mut self, body: &Value) -> (Vec<LogMessage>, Option<PortalReport>) {
        self.last_rx = Some(Instant::now());
        match replies::classify_reply(body) {
            Reply::Report(report) => {
                if let Some(app) = &report.app {
                    if app.up_time_ms.is_some() {
                        self.reported.up_time_ms = app.up_time_ms;
                    }
                    if app.version.is_some() {
                        self.reported.version = app.version.clone();
                    }
                    if app.calibrated.is_some() {
                        self.reported.calibrated = app.calibrated;
                    }
                }
                if let Some(mca) = &report.mca {
                    self.motion_control[0].ingest(mca);
                }
                if let Some(mcb) = &report.mcb {
                    self.motion_control[1].ingest(mcb);
                }
                if let Some(p) = &report.positions {
                    self.motion_control[0].reported_position = Some(p.current_a);
                    self.motion_control[1].reported_position = Some(p.current_b);
                    self.motion_control[0].reported_target = Some(p.target_a);
                    self.motion_control[1].reported_target = Some(p.target_b);
                }
                let fresh_logs = self.logger.ingest(&report.logs);
                (fresh_logs, Some(report))
            }
            // A bootloader reply means this board is sitting in its bootloader rather than running
            // the application, so there is no application state to update from it. The firmware
            // update path reads these directly from the bus; the portal model deliberately keeps
            // its last known application state rather than clearing it, because a board mid-update
            // has not lost its calibration -- it has only stopped talking about it.
            Reply::Ack(_) | Reply::Bootloader(_) | Reply::Other(_) => (Vec::new(), None),
        }
    }

    /// Apply an action's local model effect (`portalUpdateFunction`).
    pub fn apply_action_effect(&mut self, action: ActionKind) {
        match action.local_effect() {
            LocalEffect::None => {}
            LocalEffect::ZeroAxes => {
                self.pilot.reset();
                self.pilot.set_axes(glam::Vec2::ZERO);
            }
            LocalEffect::SeeThroughAxes => {
                self.pilot.reset();
                self.pilot.set_axes(glam::vec2(0.5, 0.0));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use router_proto::value::key;

    #[test]
    fn logger_dedups_consecutive_messages() {
        let mut logger = PortalLogger::default();
        let msg = LogMessage {
            message: "Switch not seen".into(),
            level: 20,
            timestamp_ms: Some(100),
        };
        logger.ingest(&[msg.clone(), msg.clone()]);
        logger.ingest(&[msg.clone()]);
        assert_eq!(logger.messages.len(), 1);
        assert_eq!(logger.messages[0].count, 3);

        let other = LogMessage {
            message: "Homed".into(),
            level: 0,
            timestamp_ms: None,
        };
        logger.ingest(&[other]);
        assert_eq!(logger.messages.len(), 2);
    }

    #[test]
    fn incoming_positions_update_motion_control() {
        let mut portal = Portal::new(1);
        let body = Value::Map(vec![(
            key("p"),
            Value::Array(vec![
                Value::from(100),
                Value::from(-200),
                Value::from(300),
                Value::from(-400),
            ]),
        )]);
        portal.process_incoming(&body);
        assert_eq!(portal.motion_control[0].reported_position, Some(100));
        assert_eq!(portal.motion_control[1].reported_position, Some(-200));
        assert_eq!(portal.motion_control[0].reported_target, Some(300));
        assert_eq!(portal.motion_control[1].reported_target, Some(-400));
    }

    #[test]
    fn profile_push_fires_on_change_only() {
        let mut mc = MotionControl::new(Axis::A);
        assert!(mc.profile_push_due().is_none(), "no push on startup");
        assert!(mc.profile_push_due().is_none());
        mc.max_velocity = 40_000;
        assert!(mc.profile_push_due().is_some());
        assert!(mc.profile_push_due().is_none());
    }

    #[test]
    fn action_effects() {
        let mut portal = Portal::new(1);
        portal.pilot.set_axes(glam::vec2(2.0, 3.0));
        portal.apply_action_effect(ActionKind::Home);
        assert_eq!(portal.pilot.axes, glam::Vec2::ZERO);
        portal.apply_action_effect(ActionKind::SeeThrough);
        assert_eq!(portal.pilot.axes, glam::vec2(0.5, 0.0));
    }
}
