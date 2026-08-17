//! The NDJSON event schema (v1) — the contract with the RouterReports
//! Node.js viewer. Every line on disk is one JSON object with envelope
//! fields `v`, `ts` (epoch ms UTC), `seq`, `type`, plus the event payload.
//! See `RouterRS/docs/report-schema.md`.

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

pub const LEVEL_STATUS: u8 = 0;
pub const LEVEL_WARNING: u8 = 10;
pub const LEVEL_ERROR: u8 = 20;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    SessionStart {
        app_version: String,
        host: String,
        config: serde_json::Value,
        verbose: bool,
    },
    SessionEnd {
        reason: String, // "clean" | "panic"
        duration_ms: u64,
        totals: Totals,
    },
    /// Runtime configuration change.
    AppConfig { config: serde_json::Value },
    /// Operator annotation from the GUI.
    Marker { label: String },

    DeviceConnect {
        col: u8,
        transport: String, // "Serial" | "TCP" | "Sim"
        endpoint: String,
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    DeviceDisconnect {
        col: u8,
        reason: String, // "io_error" | "closed" | "stall" | "shutdown"
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },

    AckTimeout {
        col: u8,
        portal: u8,
        addr: String,
        waited_ms: u32,
    },
    /// Reserved for protocol hardening stage 3 (strict ACK).
    AckNack { col: u8, portal: u8, addr: String },
    CobsError { col: u8, detail: String },
    MsgpackError {
        col: u8,
        detail: String,
        hex_prefix: String,
    },
    /// Reserved for protocol hardening stage 2 (payload CRC).
    CrcError { col: u8, expected: u16, got: u16 },

    /// Sampled full portal status (on change, else at most 1/minute).
    PortalStatus {
        col: u8,
        portal: u8,
        #[serde(skip_serializing_if = "Option::is_none")]
        uptime_ms: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        version: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        mca: Option<AxisHealthFlags>,
        #[serde(skip_serializing_if = "Option::is_none")]
        mcb: Option<AxisHealthFlags>,
    },
    /// Firmware log line (level >= 10 always; level 0 verbose only).
    /// `count` carries dedup repeats within a 5 s window.
    PortalLog {
        col: u8,
        portal: u8,
        level: u8,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        fw_ts_ms: Option<u64>,
        count: u32,
    },
    HealthTransition {
        scope: String, // "portal" | "column"
        col: u8,
        #[serde(skip_serializing_if = "Option::is_none")]
        portal: Option<u8>,
        from: String,
        to: String,
        reason: String,
        score: u8,
    },

    /// Periodic per-column aggregate (default every 10 s).
    BusStats {
        col: u8,
        window_ms: u64,
        tx: u64,
        rx: u64,
        acks: u64,
        timeouts: u64,
        cobs_errors: u64,
        msgpack_errors: u64,
        latency_ms: LatencyStats,
        outbox_peak: u32,
    },

    /// Verbose-mode raw packet events.
    PacketTx {
        col: u8,
        portal: i8,
        addr: String,
        bytes: u32,
        needs_ack: bool,
    },
    PacketRx {
        col: u8,
        source: i8,
        kind: String, // "ack" | "report" | "other"
        bytes: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        latency_ms: Option<f32>,
    },

    /// Emitted by the writer after channel overflow recovers.
    DroppedEvents { count: u64 },
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct Totals {
    pub tx: u64,
    pub rx: u64,
    pub acks: u64,
    pub timeouts: u64,
    pub cobs_errors: u64,
    pub msgpack_errors: u64,
    pub portal_error_logs: u64,
    pub reconnects: u64,
    pub dropped_events: u64,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct LatencyStats {
    pub p50: f32,
    pub p90: f32,
    pub p99: f32,
    pub max: f32,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct AxisHealthFlags {
    pub measure_cycle_ok: Option<bool>,
    pub switches_ok: Option<bool>,
    pub backlash_ok: Option<bool>,
    pub home_ok: Option<bool>,
}

impl Event {
    /// Fault events are always written raw (subject to the storm guard) and
    /// appear in the live fault feed.
    pub fn is_fault(&self) -> bool {
        matches!(
            self,
            Event::AckTimeout { .. }
                | Event::AckNack { .. }
                | Event::CobsError { .. }
                | Event::MsgpackError { .. }
                | Event::CrcError { .. }
                | Event::DeviceDisconnect { .. }
                | Event::HealthTransition { .. }
        )
    }

    /// Storm-guard key: identical keys firing >10/s collapse into one line
    /// per second with a `repeat` count.
    pub fn storm_key(&self) -> Option<(u8, u8, &'static str)> {
        match self {
            Event::AckTimeout { col, portal, .. } => Some((*col, *portal, "ack_timeout")),
            Event::CobsError { col, .. } => Some((*col, 0, "cobs_error")),
            Event::MsgpackError { col, .. } => Some((*col, 0, "msgpack_error")),
            Event::CrcError { col, .. } => Some((*col, 0, "crc_error")),
            _ => None,
        }
    }
}

/// Wrap an event with the envelope fields into the final NDJSON value.
pub fn to_line(event: &Event, ts_ms: u64, seq: u64, repeat: Option<u32>) -> serde_json::Value {
    let mut value = serde_json::to_value(event).expect("event serializes");
    let obj = value.as_object_mut().expect("event is an object");
    obj.insert("v".into(), SCHEMA_VERSION.into());
    obj.insert("ts".into(), ts_ms.into());
    obj.insert("seq".into(), seq.into());
    if let Some(repeat) = repeat {
        obj.insert("repeat".into(), repeat.into());
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_lines_are_tagged_snake_case() {
        let event = Event::AckTimeout {
            col: 3,
            portal: 12,
            addr: "m".into(),
            waited_ms: 300,
        };
        let line = to_line(&event, 1_700_000_000_000, 42, None);
        assert_eq!(line["type"], "ack_timeout");
        assert_eq!(line["v"], 1);
        assert_eq!(line["seq"], 42);
        assert_eq!(line["col"], 3);
        assert_eq!(line["portal"], 12);
        assert_eq!(line["waited_ms"], 300);
    }

    #[test]
    fn optional_fields_are_omitted() {
        let event = Event::PortalStatus {
            col: 0,
            portal: 1,
            uptime_ms: Some(1000),
            version: None,
            mca: None,
            mcb: None,
        };
        let line = to_line(&event, 0, 0, None);
        assert!(line.get("version").is_none());
        assert_eq!(line["uptime_ms"], 1000);
    }
}
