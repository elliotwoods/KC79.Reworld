//! Parsing of device -> host message bodies.
//!
//! A portal reply body is one of:
//! - a bare bool: ACK (true = success, false = firmware exception)
//! - a map with any subset of the keys `app`, `mca`, `mcb`, `logger`, `p`
//!   (the full status report has all of them; the mini position poll reply
//!   has just `p`).

use rmpv::Value;

/// Classified reply body.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    /// Bare bool ACK. The C++ app never inspects the value; we log it.
    Ack(bool),
    /// A map body parsed into a report (may be partial).
    Report(PortalReport),
    /// Anything else (unexpected).
    Other(Value),
}

/// Succinct position report `{"p": [curA, curB, tgtA, tgtB]}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Positions {
    pub current_a: i32,
    pub current_b: i32,
    pub target_a: i32,
    pub target_b: i32,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AppStatus {
    pub up_time_ms: Option<u64>,
    pub version: Option<String>,
    pub calibrated: Option<bool>,
    pub provision_serial: Option<u32>,
    pub settings_version: Option<u32>,
    pub operating_current_ma: Option<u16>,
    pub full_current_home_recovery: Option<bool>,
    pub settings_source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SettingsStatus {
    pub version: Option<u32>,
    pub operating_current_ma: Option<u16>,
    pub full_current_home_recovery: Option<bool>,
    pub source: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HealthStatus {
    pub measure_cycle_ok: Option<bool>,
    pub switches_ok: Option<bool>,
    pub backlash_ok: Option<bool>,
    pub home_ok: Option<bool>,
}

impl HealthStatus {
    pub fn all_ok(&self) -> Option<bool> {
        match (
            self.measure_cycle_ok,
            self.switches_ok,
            self.backlash_ok,
            self.home_ok,
        ) {
            (Some(a), Some(b), Some(c), Some(d)) => Some(a && b && c && d),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct MotionControlStatus {
    pub position: Option<i32>,
    pub target_position: Option<i32>,
    pub health: HealthStatus,
    pub maximum_speed: Option<i32>,
    pub acceleration: Option<i32>,
    pub minimum_speed: Option<i32>,
}

/// A firmware log message. Log levels: 0 = Status, 10 = Warning, 20 = Error.
#[derive(Debug, Clone, PartialEq)]
pub struct LogMessage {
    pub message: String,
    pub level: u8,
    /// Firmware uptime timestamp in milliseconds. The firmware sends the key
    /// `"timestamp"`; the C++ app reads `"timestamp_ms"` (a latent bug) — we
    /// accept either.
    pub timestamp_ms: Option<u64>,
}

pub const LOG_LEVEL_STATUS: u8 = 0;
pub const LOG_LEVEL_WARNING: u8 = 10;
pub const LOG_LEVEL_ERROR: u8 = 20;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PortalReport {
    pub app: Option<AppStatus>,
    pub mca: Option<MotionControlStatus>,
    pub mcb: Option<MotionControlStatus>,
    pub logs: Vec<LogMessage>,
    pub positions: Option<Positions>,
    pub settings: Option<SettingsStatus>,
}

impl PortalReport {
    pub fn is_empty(&self) -> bool {
        self.app.is_none()
            && self.mca.is_none()
            && self.mcb.is_none()
            && self.logs.is_empty()
            && self.positions.is_none()
            && self.settings.is_none()
    }
}

/// Classify a reply body from a device.
pub fn classify_reply(body: &Value) -> Reply {
    match body {
        Value::Boolean(b) => Reply::Ack(*b),
        Value::Map(_) => {
            let report = parse_report(body);
            if report.is_empty() {
                Reply::Other(body.clone())
            } else {
                Reply::Report(report)
            }
        }
        other => Reply::Other(other.clone()),
    }
}

/// Parse any subset of report keys from a map body.
pub fn parse_report(body: &Value) -> PortalReport {
    let mut report = PortalReport::default();
    let Value::Map(entries) = body else {
        return report;
    };
    for (k, v) in entries {
        match k.as_str() {
            Some("app") => report.app = Some(parse_app(v)),
            Some("mca") => report.mca = Some(parse_motion_control(v)),
            Some("mcb") => report.mcb = Some(parse_motion_control(v)),
            Some("logger") => report.logs = parse_logs(v),
            Some("p") => report.positions = parse_positions(v),
            Some("settings") => report.settings = Some(parse_settings(v)),
            _ => {}
        }
    }
    report
}

fn map_get<'a>(map: &'a Value, key: &str) -> Option<&'a Value> {
    let Value::Map(entries) = map else {
        return None;
    };
    entries
        .iter()
        .find(|(k, _)| k.as_str() == Some(key))
        .map(|(_, v)| v)
}

fn get_i32(map: &Value, key: &str) -> Option<i32> {
    map_get(map, key)?
        .as_i64()
        .and_then(|v| i32::try_from(v).ok())
}

fn get_bool(map: &Value, key: &str) -> Option<bool> {
    map_get(map, key)?.as_bool()
}

fn get_u32(map: &Value, key: &str) -> Option<u32> {
    map_get(map, key)?
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
}

fn get_u16(map: &Value, key: &str) -> Option<u16> {
    map_get(map, key)?
        .as_u64()
        .and_then(|v| u16::try_from(v).ok())
}

fn parse_app(v: &Value) -> AppStatus {
    AppStatus {
        up_time_ms: map_get(v, "upTime").and_then(|v| v.as_u64()),
        version: map_get(v, "version")
            .and_then(|v| v.as_str())
            .map(str::to_owned),
        calibrated: get_bool(v, "calibrated"),
        provision_serial: get_u32(v, "provisionSerial"),
        settings_version: get_u32(v, "settingsVersion"),
        operating_current_ma: get_u16(v, "operatingCurrentMa"),
        full_current_home_recovery: get_bool(v, "fullCurrentHomeRecovery"),
        settings_source: map_get(v, "settingsSource")
            .and_then(Value::as_str)
            .map(str::to_owned),
    }
}

fn parse_settings(v: &Value) -> SettingsStatus {
    SettingsStatus {
        version: get_u32(v, "version"),
        operating_current_ma: get_u16(v, "operatingCurrentMa"),
        full_current_home_recovery: get_bool(v, "fullCurrentHomeRecovery"),
        source: map_get(v, "source")
            .and_then(Value::as_str)
            .map(str::to_owned),
    }
}

fn parse_motion_control(v: &Value) -> MotionControlStatus {
    let health = map_get(v, "healthStatus")
        .map(|h| HealthStatus {
            measure_cycle_ok: get_bool(h, "measureCycleOK"),
            // Firmware quirk: this key is capitalized on the wire
            switches_ok: get_bool(h, "SwitchesOK").or_else(|| get_bool(h, "switchesOK")),
            backlash_ok: get_bool(h, "backlashOK"),
            home_ok: get_bool(h, "homeOK"),
        })
        .unwrap_or_default();
    MotionControlStatus {
        position: get_i32(v, "position"),
        target_position: get_i32(v, "targetPosition"),
        health,
        maximum_speed: get_i32(v, "maximumSpeed"),
        acceleration: get_i32(v, "acceleration"),
        minimum_speed: get_i32(v, "minimumSpeed"),
    }
}

fn parse_logs(v: &Value) -> Vec<LogMessage> {
    let Value::Array(items) = v else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let message = map_get(item, "message")?.as_str()?.to_owned();
            let level = map_get(item, "level")?.as_u64()? as u8;
            let timestamp_ms = map_get(item, "timestamp")
                .or_else(|| map_get(item, "timestamp_ms"))
                .and_then(|v| v.as_u64());
            Some(LogMessage {
                message,
                level,
                timestamp_ms,
            })
        })
        .collect()
}

fn parse_positions(v: &Value) -> Option<Positions> {
    let Value::Array(items) = v else { return None };
    if items.len() < 4 {
        return None;
    }
    let mut vals = [0i32; 4];
    for (i, item) in items.iter().take(4).enumerate() {
        vals[i] = i32::try_from(item.as_i64()?).ok()?;
    }
    Some(Positions {
        current_a: vals[0],
        current_b: vals[1],
        target_a: vals[2],
        target_b: vals[3],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::key;

    #[test]
    fn ack_classification() {
        assert_eq!(classify_reply(&Value::Boolean(true)), Reply::Ack(true));
        assert_eq!(classify_reply(&Value::Boolean(false)), Reply::Ack(false));
    }

    #[test]
    fn mini_position_report() {
        let body = Value::Map(vec![(
            key("p"),
            Value::Array(vec![
                Value::from(94_848),
                Value::from(0),
                Value::from(94_848),
                Value::from(0),
            ]),
        )]);
        let Reply::Report(report) = classify_reply(&body) else {
            panic!()
        };
        assert_eq!(
            report.positions,
            Some(Positions {
                current_a: 94_848,
                current_b: 0,
                target_a: 94_848,
                target_b: 0
            })
        );
    }

    #[test]
    fn full_status_report() {
        let mc = Value::Map(vec![
            (key("position"), Value::from(-100)),
            (key("targetPosition"), Value::from(200)),
            (
                key("healthStatus"),
                Value::Map(vec![
                    (key("measureCycleOK"), Value::Boolean(true)),
                    (key("SwitchesOK"), Value::Boolean(true)),
                    (key("backlashOK"), Value::Boolean(false)),
                    (key("homeOK"), Value::Boolean(true)),
                ]),
            ),
        ]);
        let body = Value::Map(vec![
            (
                key("app"),
                Value::Map(vec![
                    (key("upTime"), Value::from(123_456u64)),
                    (key("version"), Value::String("1.4.0".into())),
                ]),
            ),
            (key("mca"), mc.clone()),
            (key("mcb"), mc),
            (
                key("logger"),
                Value::Array(vec![Value::Map(vec![
                    (key("level"), Value::from(20)),
                    (key("message"), Value::String("Switch not seen".into())),
                    (key("timestamp"), Value::from(9_000u64)),
                ])]),
            ),
        ]);
        let Reply::Report(report) = classify_reply(&body) else {
            panic!()
        };
        let app = report.app.unwrap();
        assert_eq!(app.up_time_ms, Some(123_456));
        assert_eq!(app.version.as_deref(), Some("1.4.0"));
        let mca = report.mca.unwrap();
        assert_eq!(mca.position, Some(-100));
        assert_eq!(mca.target_position, Some(200));
        assert_eq!(mca.health.switches_ok, Some(true));
        assert_eq!(mca.health.all_ok(), Some(false));
        assert_eq!(report.logs.len(), 1);
        assert_eq!(report.logs[0].level, LOG_LEVEL_ERROR);
        assert_eq!(report.logs[0].timestamp_ms, Some(9_000));
    }

    #[test]
    fn unknown_body_is_other() {
        let body = Value::Map(vec![(key("unknown"), Value::Nil)]);
        assert!(matches!(classify_reply(&body), Reply::Other(_)));
    }
}
