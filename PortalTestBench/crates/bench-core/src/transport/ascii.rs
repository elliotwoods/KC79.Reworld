//! The bench firmware's ASCII line protocol.
//!
//! `HomeSwitchTest`'s `home_switch_bench` build talks lines over USART1 at 115200. Host to
//! firmware is a single letter and optional comma-separated arguments; firmware to host is a
//! comma-separated record whose first field is a one-character kind, plus `#` banner lines.
//!
//! This module is **only the parser** — pure, no I/O — so it can be tested against captured
//! streams. The formats are taken from the firmware's own `snprintf` calls in
//! `HomeSwitchTest/src/bench_main.cpp`, not from documentation:
//!
//! ```text
//! # usteps_per_rev=%ld default_threshold=%d gear_ratio=%d
//! S,<millis>,<sensorActive>,<threshold>,<pos>,<deg*10>,<running>,<enabled>,<fault>,<homed>
//! L,<level>,<message>
//! ```
//!
//! Anything else — `O,` fast-home progress, `N,` census, `K,` grid scan, `Q,` probe, `B,`
//! backlash — is carried through as [`LinkEvent::Token`] with its fields intact. Those records
//! are the raw material of the characterisation plans, and inventing a typed shape for each
//! before there is a consumer would be guessing.
//!
//! # A caution about this firmware
//!
//! The bench image is a **flat build at 0x08000000**: it overwrites the RS485 bootloader, and a
//! module carrying it cannot be field-updated until a production image is written back over
//! SWD. Anything that flashes it has to say so.

use crate::dut::{Axis, FirmwareKind, GearRatio};
use crate::transport::LinkEvent;

/// The module is wired entirely to the B channels on the 16:1 unit, and the bench rig runs
/// side A. The bench protocol is single-axis and does not name one, so every position it
/// reports is attributed to the axis the rig is wired to.
pub const BENCH_AXIS: Axis = Axis::A;

/// Parse one line from the bench firmware.
///
/// Returns `None` for blank lines and for records that carry no information (rather than
/// inventing an event for them). Malformed records become a [`LinkEvent::Fault`] so they are
/// recorded rather than silently dropped — a stream that has started producing garbage is
/// itself a finding.
pub fn parse_line(line: &str) -> Option<Vec<LinkEvent>> {
    let line = line.trim_end_matches(['\r', '\n']).trim();
    if line.is_empty() {
        return None;
    }

    if let Some(rest) = line.strip_prefix('#') {
        return Some(vec![parse_banner(rest.trim(), line)]);
    }

    let mut fields = line.split(',');
    let kind = fields.next()?.trim();
    let mut chars = kind.chars();
    let (Some(kind_char), None) = (chars.next(), chars.next()) else {
        // Not a record at all: the firmware also prints free-form help text.
        return None;
    };
    let fields: Vec<String> = fields.map(|f| f.trim().to_string()).collect();

    match kind_char {
        'S' => Some(parse_status(&fields, line)),
        // Parsed from the raw line rather than the trimmed fields: a log message is prose, and
        // splitting it on commas and rejoining loses the spaces after them.
        'L' => Some(vec![parse_log(line)]),
        _ => Some(vec![LinkEvent::Token {
            kind: kind_char,
            fields,
        }]),
    }
}

/// `# usteps_per_rev=189704 default_threshold=220 gear_ratio=32`
///
/// The host adopts **any** `#` line carrying `usteps_per_rev=`, because the firmware reprints
/// the banner when it auto-detects the gearing mid-session. A cached value from the first
/// banner would be wrong for the rest of the run on a module whose ratio was only discovered
/// during its first home.
fn parse_banner(rest: &str, whole: &str) -> LinkEvent {
    let mut usteps_per_rev = None;
    let mut ratio = GearRatio::Unknown;

    for token in rest.split_whitespace() {
        let Some((key, value)) = token.split_once('=') else {
            continue;
        };
        match key {
            "usteps_per_rev" => usteps_per_rev = value.parse::<i32>().ok(),
            "gear_ratio" => {
                ratio = match value.parse::<i32>() {
                    Ok(16) => GearRatio::R16,
                    Ok(32) => GearRatio::R32,
                    _ => GearRatio::Unknown,
                }
            }
            _ => {}
        }
    }

    LinkEvent::Identified {
        firmware: FirmwareKind::Bench,
        // The bench build carries no version string; the banner is the identity.
        version: None,
        ratio,
        usteps_per_rev,
        banner: whole.to_string(),
    }
}

/// `S,<millis>,<sensorActive>,<threshold>,<pos>,<deg*10>,<running>,<enabled>,<fault>,<homed>`
fn parse_status(fields: &[String], whole: &str) -> Vec<LinkEvent> {
    // Nine fields follow the kind. Fewer means a truncated line -- a real occurrence on a
    // serial link, and one worth recording rather than half-parsing.
    if fields.len() < 9 {
        return vec![LinkEvent::Fault(format!("short status record: {whole}"))];
    }

    let number = |i: usize| fields[i].parse::<i64>().ok();
    let flag = |i: usize| number(i).map(|v| v != 0);

    let mut events = Vec::with_capacity(4);

    if let Some(ms) = number(0) {
        events.push(LinkEvent::Uptime {
            seconds: (ms / 1000) as i32,
        });
    }
    if let (Some(active), Some(threshold)) = (flag(1), number(2)) {
        events.push(LinkEvent::Sensor {
            active,
            threshold: threshold as i32,
        });
    }
    if let Some(position) = number(3) {
        // The bench firmware reports where the axis *is*; it has no notion of a commanded
        // target, so `target` stays `None` rather than being faked equal to the position.
        events.push(LinkEvent::Position {
            axis: BENCH_AXIS,
            position: position as i32,
            target: None,
        });
    }
    if let Some(true) = flag(7) {
        events.push(LinkEvent::Fault("motor driver reports a fault".into()));
    }

    events
}

/// `L,<level>,<message>`
///
/// Split at most three ways and take the remainder verbatim. The message routinely contains
/// commas — "cal failed, band too narrow, retrying" — and reconstructing it from trimmed
/// fields silently rewrites exactly the lines most worth reading.
fn parse_log(line: &str) -> LinkEvent {
    let mut parts = line.splitn(3, ',');
    let _kind = parts.next();
    let Some(level) = parts.next() else {
        return LinkEvent::Fault(format!("empty log record: {line}"));
    };
    LinkEvent::Log {
        level: level.trim().parse::<u8>().unwrap_or(0),
        message: parts.next().unwrap_or("").to_string(),
        firmware_ms: None,
    }
}

/// Render an [`Op`](crate::transport::Op) as a bench command line.
///
/// Returns `None` for ops this dialect cannot express; the caller turns that into
/// [`LinkError::Unsupported`](crate::transport::LinkError::Unsupported).
pub fn render_op(op: &crate::transport::Op) -> Option<String> {
    use crate::transport::Op;
    Some(match op {
        // `P` asks for one status line now; the banner is reprinted by `V`.
        Op::Identify => "V".to_string(),
        // The bench build has no composite startup routine; `O` is its whole homing story.
        Op::Startup => return None,
        Op::Poll | Op::PollPosition => "P".to_string(),
        // `O` is the fast home + backlash routine -- the production-candidate one, and the
        // only homing worth measuring on this rig.
        Op::Home { axis } if *axis == BENCH_AXIS => "O".to_string(),
        Op::Home { .. } => return None,
        Op::SetHomeThreshold { value } => format!("T,{value}"),
        // `N [T] [vmax] [accel] [M]` -- the census this whole threshold rule was derived from.
        // The bench rig wires one axis only (`BENCH_AXIS`), so a census naming the other one is
        // refused rather than silently redirected.
        Op::Census {
            axis,
            threshold,
            speed,
        } if *axis == BENCH_AXIS => match speed {
            Some(speed) => format!("N,{threshold},{speed}"),
            None => format!("N,{threshold}"),
        },
        Op::Census { .. } => return None,
        Op::MoveTo { axis, usteps, .. } if *axis == BENCH_AXIS => format!("G,{usteps}"),
        Op::MoveTo { .. } | Op::MoveAxes { .. } | Op::SetMotionProfile { .. } => return None,
        Op::Escape => "X".to_string(),
        Op::SetMicrostep { resolution } => format!("U,{resolution}"),
        // The bench build has no unjam, no per-axis calibrate, no backlash-only routine (`O`
        // measures backlash as part of homing), no current command and no soft reboot.
        Op::Unjam { .. }
        | Op::Calibrate { .. }
        | Op::MeasureBacklash { .. }
        | Op::SetCurrent { .. }
        | Op::ReadSettings
        | Op::EnterDirect
        | Op::ExitDirect
        | Op::DirectHeartbeat
        | Op::Jog { .. }
        | Op::Survey { .. }
        | Op::WriteSettings { .. }
        | Op::Reboot => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_carries_the_measured_gearing() {
        let events =
            parse_line("# usteps_per_rev=189704 default_threshold=220 gear_ratio=32").unwrap();
        match &events[0] {
            LinkEvent::Identified {
                firmware,
                ratio,
                usteps_per_rev,
                ..
            } => {
                assert_eq!(*firmware, FirmwareKind::Bench);
                assert_eq!(*ratio, GearRatio::R32);
                assert_eq!(*usteps_per_rev, Some(189_704));
            }
            other => panic!("expected Identified, got {other:?}"),
        }
    }

    /// The firmware reprints the banner when it auto-detects a 16:1 module, and the host must
    /// adopt the new figure. 92,252 is measured; the nominal 94,852 is 2.8% wrong.
    #[test]
    fn a_reprinted_banner_reports_the_16_to_1_gearing() {
        let events =
            parse_line("# usteps_per_rev=92252 default_threshold=230 gear_ratio=16").unwrap();
        match &events[0] {
            LinkEvent::Identified {
                ratio,
                usteps_per_rev,
                ..
            } => {
                assert_eq!(*ratio, GearRatio::R16);
                assert_eq!(*usteps_per_rev, Some(92_252));
            }
            other => panic!("expected Identified, got {other:?}"),
        }
    }

    #[test]
    fn status_line_yields_uptime_sensor_and_position() {
        // millis, sensorActive, threshold, pos, deg*10, running, enabled, fault, homed
        let events = parse_line("S,125000,1,246,47426,900,0,1,0,1").unwrap();
        assert!(events.contains(&LinkEvent::Uptime { seconds: 125 }));
        assert!(events.contains(&LinkEvent::Sensor {
            active: true,
            threshold: 246
        }));
        assert!(events.contains(&LinkEvent::Position {
            axis: BENCH_AXIS,
            position: 47_426,
            target: None,
        }));
    }

    /// The bench firmware has no commanded target, so one must not be invented. A target equal
    /// to the position would draw as a perfectly-tracking axis, which is a lie.
    #[test]
    fn status_line_does_not_invent_a_target() {
        let events = parse_line("S,1000,0,246,100,2,0,1,0,0").unwrap();
        let position = events
            .iter()
            .find_map(|e| match e {
                LinkEvent::Position { target, .. } => Some(*target),
                _ => None,
            })
            .expect("a position event");
        assert_eq!(position, None);
    }

    #[test]
    fn a_driver_fault_flag_becomes_a_fault_event() {
        let events = parse_line("S,1000,0,246,100,2,0,1,1,0").unwrap();
        assert!(events.iter().any(|e| matches!(e, LinkEvent::Fault(_))));
    }

    #[test]
    fn a_truncated_status_line_is_recorded_not_half_parsed() {
        let events = parse_line("S,1000,0,246").unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], LinkEvent::Fault(m) if m.contains("short status")));
    }

    /// Log messages contain commas often enough that splitting on them and keeping the first
    /// piece would quietly truncate exactly the messages worth reading.
    #[test]
    fn log_messages_keep_their_commas() {
        let events = parse_line("L,1,cal failed, band too narrow, retrying").unwrap();
        assert_eq!(
            events[0],
            LinkEvent::Log {
                level: 1,
                message: "cal failed, band too narrow, retrying".into(),
                firmware_ms: None,
            }
        );
    }

    /// The characterisation records are carried through with their fields intact rather than
    /// being given a guessed shape before anything consumes them.
    #[test]
    fn unknown_record_kinds_survive_as_tokens() {
        let events = parse_line("O,done,47426,530").unwrap();
        assert_eq!(
            events[0],
            LinkEvent::Token {
                kind: 'O',
                fields: vec!["done".into(), "47426".into(), "530".into()],
            }
        );
    }

    #[test]
    fn blank_lines_and_free_text_are_ignored() {
        assert!(parse_line("").is_none());
        assert!(parse_line("   \r\n").is_none());
        assert!(parse_line("help: T=threshold M=mode").is_none());
    }

    #[test]
    fn ops_the_bench_dialect_cannot_express_are_refused_rather_than_approximated() {
        use crate::transport::Op;
        assert_eq!(render_op(&Op::Home { axis: Axis::A }).as_deref(), Some("O"));
        assert_eq!(
            render_op(&Op::SetHomeThreshold { value: 246 }).as_deref(),
            Some("T,246")
        );
        assert_eq!(render_op(&Op::Escape).as_deref(), Some("X"));
        // There is no unjam on this build. Silently sending something else would move the
        // motor in a way the operator did not ask for.
        assert!(render_op(&Op::Unjam { axis: Axis::A }).is_none());
        assert!(render_op(&Op::SetCurrent { amps: 0.15 }).is_none());
    }
}
