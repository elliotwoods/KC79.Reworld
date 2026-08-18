//! The RS485 link to production firmware.
//!
//! Built on [`router_link::rs485`], which carries the wire behaviour of the original C++ Router
//! bit-for-bit: a 300 ms source-ID ACK window, 100 ms between broadcast sends, 5 ms after the
//! last receive, and outbox collation that keeps only the newest packet per address. Those are
//! not tuning knobs. Changing one makes every bus measurement taken with a different value
//! incomparable, which is most of the point of measuring.
//!
//! # One module, not an installation
//!
//! The Router addresses columns of portals; this bench addresses **one module**. So there is a
//! single column-0 bus and a single target id, defaulting to 1. Broadcast (`-1`) is used only
//! for [`Op::Identify`], because a bench module's id is often exactly what is unknown — the
//! daisy-chain that assigns ids may not be connected on a desk.
//!
//! # The log drains when you read it
//!
//! `Op::Poll` asks for the full status report, and the firmware's `logger` field **empties its
//! outbox** as it serialises. Those log lines exist nowhere else afterwards. That is why
//! [`Op::PollPosition`] exists as a separate, cheap op: a live position display can run at
//! whatever rate it likes without consuming the log, and the full poll can run slowly.

use crate::dut::{Axis, FirmwareKind, GearRatio, Health};
use crate::transport::{Link, LinkError, LinkEvent, LinkInfo, LinkKind, Op};
use router_link::rs485::{Packet, Rs485};
use router_proto::commands;
use router_proto::replies::{classify_reply, MotionControlStatus, PortalReport, Reply};

/// Default address of the module under test.
///
/// Ids are assigned over the ID daisy-chain, which a module on a desk usually has no neighbour
/// for; 1 is what a lone module adopts.
pub const DEFAULT_TARGET: i8 = 1;

pub struct Rs485Link {
    kind: LinkKind,
    endpoint: String,
    target: i8,
    bus: Rs485,
    banner: Option<String>,
    opened: bool,
}

impl Rs485Link {
    /// `endpoint` is `COM15` for [`LinkKind::Rs485Serial`] or `192.168.1.201:4196` for
    /// [`LinkKind::Rs485Tcp`].
    pub fn new(kind: LinkKind, endpoint: impl Into<String>, target: i8) -> Self {
        Self {
            kind,
            endpoint: endpoint.into(),
            target,
            // Reporting is attached by the worker, which owns the session file. A link built
            // for a one-off command should not open a session of its own.
            bus: Rs485::new(0, router_report::Reporter::disabled()),
            banner: None,
            opened: false,
        }
    }

    /// Attach a reporter so bus traffic, ACK timeouts and framing errors reach the session file.
    pub fn with_reporter(mut self, reporter: router_report::Reporter) -> Self {
        self.bus = Rs485::new(0, reporter);
        self
    }

    fn settings(&self) -> serde_json::Value {
        match self.kind {
            LinkKind::Rs485Tcp => {
                let (address, port) = self.endpoint.split_once(':').unwrap_or((&self.endpoint, "4196"));
                serde_json::json!({
                    "deviceType": "TCP",
                    "address": address,
                    "port": port.parse::<u16>().unwrap_or(4196),
                })
            }
            _ => serde_json::json!({ "deviceType": "Serial", "address": self.endpoint }),
        }
    }
}

/// Turn one op into a body and the collation address the outbox keys on.
///
/// The address matters: two `move` commands queued back to back should collapse to the newest,
/// but a `home` and a `move` must not. Sharing the C++ address strings keeps that behaviour
/// identical to the Router's.
fn render(op: &Op, target: i8) -> Result<(i8, router_proto::Value, &'static str), LinkError> {
    use router_proto::commands::MeasureSettings;

    let unsupported = |op: &Op| LinkError::Unsupported { kind: "rs485", op: format!("{op:?}") };

    Ok(match op {
        // Broadcast: a module on a desk may not have been given an id yet.
        Op::Identify => (router_proto::BROADCAST, commands::poll(), "poll"),
        Op::Poll => (target, commands::poll(), "poll"),
        Op::PollPosition => (target, commands::poll_position(), "p"),
        Op::Home { axis } => (
            target,
            commands::mc_home(wire_axis(*axis), MeasureSettings::default()),
            "mc/home",
        ),
        Op::MeasureBacklash { axis } => (
            target,
            commands::mc_measure_backlash(wire_axis(*axis), MeasureSettings::default()),
            "mc/measureBacklash",
        ),
        Op::MoveTo { axis, usteps, profile } => (
            target,
            match profile {
                // All four arguments together, or none: see `MotionProfile`. The firmware keeps
                // whatever profile it currently holds when only a target is sent, which is the
                // right behaviour for a jog and the wrong one for a measurement.
                Some(p) => commands::mc_move_with_profile(
                    wire_axis(*axis),
                    *usteps,
                    p.max_velocity,
                    p.acceleration,
                    p.min_velocity,
                ),
                None => commands::mc_move(wire_axis(*axis), *usteps),
            },
            "mc/move",
        ),
        Op::SetCurrent { amps } => (
            target,
            commands::mds_set_current(*amps),
            "motorDriverSettings/setCurrent",
        ),
        Op::SetMicrostep { resolution } => (
            target,
            commands::mds_set_microstep_resolution(*resolution),
            "motorDriverSettings/setMicrostepResolution",
        ),
        Op::SetHomeThreshold { value } => (target, commands::home_threshold(*value), "homeThreshold"),
        Op::Startup => (target, action_body("init")?, "init"),
        Op::Unjam { .. } => (target, action_body("unjam")?, "unjam"),
        Op::Calibrate { .. } => (target, action_body("calibrate")?, "calibrate"),
        Op::Escape => (target, action_body("escapeFromRoutine")?, "escapeFromRoutine"),
        Op::Reboot => (target, action_body("reboot")?, "reboot"),
        #[allow(unreachable_patterns, reason = "guards the match if Op gains a variant")]
        other => return Err(unsupported(other)),
    })
}

/// The firmware's whole-module actions are keyed by OSC-style name in `ActionKind`.
///
/// Note these are **both-axis** on the firmware side: `unjam` and `calibrate` run A then B.
/// The op names an axis because the bench vocabulary is per-axis, but the wire cannot express
/// that, and pretending otherwise would report a single-axis result for a two-axis motion.
fn action_body(osc: &str) -> Result<router_proto::Value, LinkError> {
    commands::ActionKind::ALL
        .iter()
        .find(|action| action.osc_address() == osc)
        .map(|action| action.body())
        .ok_or_else(|| LinkError::Unsupported { kind: "rs485", op: osc.to_string() })
}

/// The bench's axis type, as the protocol's own.
///
/// Two enums rather than one because the protocol crate is shared with RouterRS and should not
/// grow a dependency on this crate's vocabulary.
fn wire_axis(axis: Axis) -> commands::Axis {
    match axis {
        Axis::A => commands::Axis::A,
        Axis::B => commands::Axis::B,
    }
}

// Bodies stay as MessagePack values all the way to the outbox: `Packet::from_body` encodes
// them with the same minimal-int rules the C++ Router used, and round-tripping through JSON
// would risk changing an integer's width on the wire.

fn events_from_report(report: &PortalReport) -> Vec<LinkEvent> {
    let mut events = Vec::new();

    if let Some(app) = &report.app {
        if let Some(ms) = app.up_time_ms {
            events.push(LinkEvent::Uptime { seconds: (ms / 1000) as i32 });
        }
        if let Some(version) = &app.version {
            events.push(LinkEvent::Identified {
                firmware: FirmwareKind::Production,
                version: Some(version.clone()),
                // The report does not carry the gearing; it is learned from the home routine's
                // own log line, or from the bench link's banner.
                ratio: GearRatio::Unknown,
                usteps_per_rev: None,
                banner: version.clone(),
            });
        }
    }

    for (axis, status) in [(Axis::A, &report.mca), (Axis::B, &report.mcb)] {
        let Some(status) = status else { continue };
        events.extend(events_from_motion_control(axis, status));
    }

    if let Some(positions) = &report.positions {
        events.push(LinkEvent::Position {
            axis: Axis::A,
            position: positions.current_a,
            target: Some(positions.target_a),
        });
        events.push(LinkEvent::Position {
            axis: Axis::B,
            position: positions.current_b,
            target: Some(positions.target_b),
        });
    }

    for log in &report.logs {
        events.push(LinkEvent::Log {
            level: log.level,
            message: log.message.clone(),
            firmware_ms: log.timestamp_ms,
        });
    }

    events
}

fn events_from_motion_control(axis: Axis, status: &MotionControlStatus) -> Vec<LinkEvent> {
    let mut events = Vec::new();

    if let Some(position) = status.position {
        events.push(LinkEvent::Position { axis, position, target: status.target_position });
    }

    // Every flag is an `Option`: a report that omitted one must not be read as `false`, which
    // would draw a healthy module as faulty. Only report health once the firmware has actually
    // said something about all four.
    let h = &status.health;
    if let (Some(measure_cycle_ok), Some(switches_ok), Some(backlash_ok), Some(home_ok)) =
        (h.measure_cycle_ok, h.switches_ok, h.backlash_ok, h.home_ok)
    {
        events.push(LinkEvent::HealthReport {
            axis,
            health: Health { measure_cycle_ok, switches_ok, backlash_ok, home_ok },
        });
    }

    events
}

impl Link for Rs485Link {
    fn info(&self) -> LinkInfo {
        LinkInfo { kind: self.kind, endpoint: self.endpoint.clone(), banner: self.banner.clone() }
    }

    fn open(&mut self) -> Result<LinkInfo, LinkError> {
        self.bus.open_from_settings(self.settings());
        self.opened = true;
        Ok(self.info())
    }

    fn close(&mut self) {
        self.bus.close();
        self.opened = false;
    }

    fn is_open(&self) -> bool {
        self.opened && self.bus.is_connected()
    }

    fn send(&mut self, op: &Op) -> Result<(), LinkError> {
        if !self.opened {
            return Err(LinkError::NotOpen);
        }
        let (target, body, address) = render(op, self.target)?;
        self.bus.transmit(Packet::from_body(target, &body, address));
        Ok(())
    }

    fn poll(&mut self, _now_ms: u64) -> Vec<LinkEvent> {
        if !self.opened {
            return Vec::new();
        }
        let mut events = Vec::new();
        for envelope in self.bus.update() {
            match classify_reply(&envelope.body) {
                Reply::Ack(_) => events.push(LinkEvent::Ack),
                Reply::Report(report) => {
                    for event in events_from_report(&report) {
                        if let LinkEvent::Identified { banner, .. } = &event {
                            self.banner = Some(banner.clone());
                        }
                        events.push(event);
                    }
                }
                Reply::Other(value) => {
                    events.push(LinkEvent::Fault(format!("unexpected reply: {value}")))
                }
            }
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use router_proto::replies::{AppStatus, HealthStatus};

    #[test]
    fn identify_broadcasts_because_a_bench_module_may_have_no_id_yet() {
        let (target, _, address) = render(&Op::Identify, DEFAULT_TARGET).unwrap();
        assert_eq!(target, router_proto::BROADCAST);
        assert_eq!(address, "poll");

        // Everything else is addressed to the module.
        let (target, _, _) = render(&Op::Poll, 7).unwrap();
        assert_eq!(target, 7);
    }

    /// Collation keeps the newest packet per address, so two ops that must not replace each
    /// other need different addresses. A `move` overwriting a queued `home` would silently drop
    /// the homing a plan depends on.
    #[test]
    fn distinct_operations_use_distinct_collation_addresses() {
        let addresses: Vec<&str> = [
            Op::Poll,
            Op::PollPosition,
            Op::Home { axis: Axis::A },
            Op::MoveTo { axis: Axis::A, usteps: 0, profile: None },
            Op::MeasureBacklash { axis: Axis::A },
            Op::SetCurrent { amps: 0.15 },
        ]
        .iter()
        .map(|op| render(op, DEFAULT_TARGET).unwrap().2)
        .collect();

        let unique: std::collections::HashSet<_> = addresses.iter().collect();
        assert_eq!(unique.len(), addresses.len(), "collation addresses collide: {addresses:?}");
    }

    #[test]
    fn the_whole_module_actions_resolve_to_real_firmware_commands() {
        for op in [Op::Unjam { axis: Axis::A }, Op::Calibrate { axis: Axis::B }, Op::Escape, Op::Reboot] {
            assert!(render(&op, DEFAULT_TARGET).is_ok(), "{op:?} did not render");
        }
    }

    /// A report that omits a health flag must not be read as a failure. The firmware sends
    /// nothing for an axis it has not measured, and drawing that as "not ok" sends someone
    /// looking for a fault that was never reported.
    #[test]
    fn a_partial_health_report_produces_no_health_event() {
        let status = MotionControlStatus {
            position: Some(100),
            target_position: Some(100),
            health: HealthStatus { home_ok: Some(true), ..Default::default() },
            ..Default::default()
        };
        let events = events_from_motion_control(Axis::A, &status);
        assert!(events.iter().any(|e| matches!(e, LinkEvent::Position { .. })));
        assert!(
            !events.iter().any(|e| matches!(e, LinkEvent::HealthReport { .. })),
            "a partial report must not be completed with guesses: {events:?}"
        );
    }

    #[test]
    fn a_complete_health_report_carries_all_four_flags() {
        let status = MotionControlStatus {
            health: HealthStatus {
                measure_cycle_ok: Some(true),
                switches_ok: Some(true),
                backlash_ok: Some(false),
                home_ok: Some(true),
            },
            ..Default::default()
        };
        let events = events_from_motion_control(Axis::B, &status);
        let health = events
            .iter()
            .find_map(|e| match e {
                LinkEvent::HealthReport { health, .. } => Some(*health),
                _ => None,
            })
            .expect("a health event");
        assert!(!health.backlash_ok);
        assert!(!health.all_ok());
    }

    #[test]
    fn a_full_report_yields_identity_uptime_positions_and_logs() {
        let report = PortalReport {
            app: Some(AppStatus {
                up_time_ms: Some(125_000),
                version: Some("Portal v2026-08-18 a94ae48".into()),
                calibrated: None,
            }),
            mca: None,
            mcb: None,
            logs: vec![router_proto::replies::LogMessage {
                message: "home ok".into(),
                level: 0,
                timestamp_ms: Some(124_000),
            }],
            positions: Some(router_proto::replies::Positions {
                current_a: 10,
                current_b: 20,
                target_a: 11,
                target_b: 21,
            }),
        };

        let events = events_from_report(&report);
        assert!(events.contains(&LinkEvent::Uptime { seconds: 125 }));
        assert!(events.iter().any(|e| matches!(
            e,
            LinkEvent::Identified { firmware: FirmwareKind::Production, .. }
        )));
        // Unlike the bench dialect, RS485 does carry a commanded target.
        assert!(events.contains(&LinkEvent::Position { axis: Axis::A, position: 10, target: Some(11) }));
        assert!(events.iter().any(|e| matches!(e, LinkEvent::Log { level: 0, .. })));
    }
}
