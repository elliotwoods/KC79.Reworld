//! Live diagnostics state published by the writer thread for the GUI
//! Diagnostics panel (refreshed ~1 Hz, read lock-free by cloning an Arc).

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ColumnState {
    #[default]
    Disconnected,
    Connected,
    /// Device open and transmitting but nothing received for a while.
    Stalled,
    /// Decode error rate above threshold (line noise / bad wiring).
    Noisy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PortalState {
    #[default]
    Unknown,
    Ok,
    Degraded,
    Faulty,
    /// Polled but not answering.
    Silent,
}

impl PortalState {
    pub fn as_str(self) -> &'static str {
        match self {
            PortalState::Unknown => "unknown",
            PortalState::Ok => "ok",
            PortalState::Degraded => "degraded",
            PortalState::Faulty => "faulty",
            PortalState::Silent => "silent",
        }
    }
}

impl ColumnState {
    pub fn as_str(self) -> &'static str {
        match self {
            ColumnState::Disconnected => "disconnected",
            ColumnState::Connected => "connected",
            ColumnState::Stalled => "stalled",
            ColumnState::Noisy => "noisy",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ColumnDiag {
    pub col: u8,
    pub state: ColumnState,
    pub endpoint: String,
    pub tx: u64,
    pub rx: u64,
    pub acks: u64,
    pub timeouts: u64,
    pub cobs_errors: u64,
    pub msgpack_errors: u64,
    pub reconnects: u64,
    pub latency_p50_ms: f32,
    pub latency_p90_ms: f32,
    pub latency_p99_ms: f32,
    pub last_rx_age_ms: Option<u64>,
    pub outbox_peak: u32,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PortalDiag {
    pub col: u8,
    pub portal: u8,
    pub state: PortalState,
    /// 0-100 health score.
    pub score: u8,
    pub ack_rate: f32,
    pub latency_p90_ms: f32,
    pub sends: u64,
    pub timeouts: u64,
    pub last_seen_age_ms: Option<u64>,
    pub error_logs: u64,
    pub warning_logs: u64,
    pub version: Option<String>,
    pub uptime_ms: Option<u64>,
    pub reboots: u32,
    /// [axis A, axis B]: measure/switches/backlash/home flags all ok?
    pub calibration_ok: [Option<bool>; 2],
}

#[derive(Debug, Clone, Serialize)]
pub struct FaultLine {
    pub ts_ms: u64,
    pub kind: String,
    pub col: u8,
    pub portal: Option<u8>,
    pub detail: String,
    pub repeat: u32,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct DiagnosticsSnapshot {
    pub session_file: String,
    pub session_start_ms: u64,
    pub file_bytes: u64,
    pub dropped_events: u64,
    pub verbose: bool,
    pub columns: Vec<ColumnDiag>,
    pub portals: Vec<PortalDiag>,
    /// Most recent faults, newest last (ring of 500).
    pub recent_faults: Vec<FaultLine>,
}
