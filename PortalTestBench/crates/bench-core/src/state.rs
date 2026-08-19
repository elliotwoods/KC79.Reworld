//! The mirror of what the bench knows.
//!
//! The worker thread owns the link and writes here; everything else — GUI parameters, the HTTP
//! routes, the NDJSON writer — reads. Nothing outside the worker ever touches hardware, which
//! is what keeps [`crate::transport::Link::poll`] a single reader (see the transport module
//! docs for why that matters).
//!
//! Every field that can be "not measured yet" is an `Option`. Drawing an unread value as zero
//! is worse than drawing a blank, because a zero looks like a measurement.

use serde::Serialize;
use std::collections::VecDeque;

use crate::dut::{Axis, FirmwareKind, GearRatio, Health};
use crate::threshold::Band;
use crate::transport::{Channel, LinkDiagnostics, LinkKind, MotionProfile};

/// How many log lines the bench keeps in memory.
///
/// The session NDJSON is the durable record; this is only what a page or `ptb log` can scroll
/// back through. The firmware's own history is 32 entries and it drains on read, so anything
/// beyond this has already been written to disk or never existed.
pub const LOG_CAPACITY: usize = 2000;

#[derive(Debug, Clone, Default, Serialize)]
pub struct AxisState {
    pub position: Option<i32>,
    pub target: Option<i32>,
    pub health: Option<Health>,
}

impl AxisState {
    /// Distance still to travel, when both ends are known.
    pub fn error(&self) -> Option<i32> {
        Some(self.target? - self.position?)
    }
}

/// What is known about the module under test.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DutState {
    /// True once anything has answered on the link.
    pub present: bool,
    pub firmware: FirmwareKind,
    pub version: Option<String>,
    pub uptime_s: Option<i32>,
    pub ratio: GearRatio,
    pub usteps_per_rev: Option<i32>,
    pub banner: Option<String>,
    pub a: AxisState,
    pub b: AxisState,
    /// The optical threshold band measured this session, if any.
    pub threshold: Option<ThresholdState>,
    pub provision_serial: Option<u32>,
    pub operating_current_ma: Option<u16>,
    pub full_current_home_recovery: Option<bool>,
    pub settings_source: Option<String>,
}

impl DutState {
    pub fn axis(&self, axis: Axis) -> &AxisState {
        match axis {
            Axis::A => &self.a,
            Axis::B => &self.b,
        }
    }

    pub fn axis_mut(&mut self, axis: Axis) -> &mut AxisState {
        match axis {
            Axis::A => &mut self.a,
            Axis::B => &mut self.b,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ThresholdState {
    pub floor: u8,
    pub shoulder: u8,
    pub band: u16,
    pub applied: u8,
    /// Seconds into the session when this was measured.
    pub calibrated_at_s: i32,
}

impl ThresholdState {
    pub fn from_band(band: Band, at_s: i32) -> Self {
        Self {
            floor: band.floor,
            shoulder: band.shoulder,
            band: band.width(),
            applied: band.operating(),
            calibrated_at_s: at_s,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct LinkState {
    pub kind: Option<LinkKind>,
    pub endpoint: Option<String>,
    pub connected: bool,
    pub detail: Option<String>,
}

/// One independently connected communication lane and the evidence observed on it.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ChannelState {
    pub link: LinkState,
    pub dut: DutState,
    /// RS485 source IDs heard since the last discovery reset. Empty for serial.
    pub discovered: Vec<i8>,
    pub selected_target: Option<i8>,
    pub diagnostics: LinkDiagnostics,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ChannelsState {
    pub serial: ChannelState,
    pub rs485: ChannelState,
}

// `LinkKind` is an enum of the transports; serialise it by name so a report reads the same
// after the variants are reordered.
impl Serialize for LinkKind {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LogLine {
    /// Milliseconds since the session started.
    pub at_ms: u64,
    pub level: u8,
    pub message: String,
    /// Where it came from: `firmware`, `bench`, or `fault`.
    pub source: &'static str,
    /// Monotonic, so `?from=` can resume without re-reading.
    pub seq: u64,
}

/// A bounded log the page and the CLI both read from a cursor.
#[derive(Debug, Default)]
pub struct LogRing {
    lines: VecDeque<LogLine>,
    next_seq: u64,
    /// Lines dropped because the ring filled. Reported rather than hidden: a gap the reader
    /// does not know about turns a partial record into a wrong one.
    pub dropped: u64,
}

/// One real position observation. Velocity and acceleration are derived from these timestamps;
/// they are never presented as firmware measurements.
#[derive(Debug, Clone, Serialize)]
pub struct TelemetrySample {
    pub seq: u64,
    pub at_ms: u64,
    pub channel: Channel,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_id: Option<i8>,
    pub axis: Axis,
    pub position: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<i32>,
}

pub const TELEMETRY_CAPACITY: usize = 8_192;

#[derive(Debug, Default)]
pub struct TelemetryRing {
    samples: VecDeque<TelemetrySample>,
    next_seq: u64,
    pub dropped: u64,
}

impl TelemetryRing {
    pub fn push(
        &mut self,
        at_ms: u64,
        channel: Channel,
        target_id: Option<i8>,
        axis: Axis,
        position: i32,
        target: Option<i32>,
    ) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.samples.push_back(TelemetrySample {
            seq,
            at_ms,
            channel,
            target_id,
            axis,
            position,
            target,
        });
        while self.samples.len() > TELEMETRY_CAPACITY {
            self.samples.pop_front();
            self.dropped += 1;
        }
        seq
    }

    pub fn since(&self, from: u64) -> Vec<&TelemetrySample> {
        self.samples
            .iter()
            .filter(|sample| sample.seq >= from)
            .collect()
    }

    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }
}

impl LogRing {
    pub fn push(&mut self, at_ms: u64, level: u8, source: &'static str, message: String) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.lines.push_back(LogLine {
            at_ms,
            level,
            message,
            source,
            seq,
        });
        while self.lines.len() > LOG_CAPACITY {
            self.lines.pop_front();
            self.dropped += 1;
        }
        seq
    }

    /// Every line with `seq >= from`, oldest first.
    pub fn since(&self, from: u64) -> Vec<&LogLine> {
        self.lines.iter().filter(|line| line.seq >= from).collect()
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }
}

/// Everything the bench currently believes, in one place.
#[derive(Debug, Clone, Default, Serialize)]
pub struct BenchState {
    /// Compatibility mirror of the active channel.
    pub link: LinkState,
    /// Compatibility mirror of the active channel's module.
    pub dut: DutState,
    pub channels: ChannelsState,
    pub active_channel: Channel,
    /// The motion profile the bench applies to moves it originates.
    pub profile: MotionProfile,
    pub faults: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_axis_with_nothing_measured_reports_no_error_rather_than_zero() {
        let axis = AxisState::default();
        assert_eq!(axis.error(), None);

        // A position with no target is still not an error of zero: nothing has been commanded.
        let axis = AxisState {
            position: Some(100),
            target: None,
            health: None,
        };
        assert_eq!(axis.error(), None);

        let axis = AxisState {
            position: Some(100),
            target: Some(140),
            health: None,
        };
        assert_eq!(axis.error(), Some(40));
    }

    #[test]
    fn the_threshold_state_records_the_band_it_came_from() {
        // The measured production ring: floor 240, shoulder 252.
        let band = Band::new(240, 252).unwrap();
        let state = ThresholdState::from_band(band, 12);
        assert_eq!(state.floor, 240);
        assert_eq!(state.band, 13);
        assert!((246..=248).contains(&state.applied));
        assert_eq!(state.calibrated_at_s, 12);
    }

    #[test]
    fn the_log_ring_resumes_from_a_cursor() {
        let mut ring = LogRing::default();
        for i in 0..5 {
            ring.push(i, 0, "firmware", format!("line {i}"));
        }
        let tail = ring.since(3);
        assert_eq!(tail.len(), 2);
        assert_eq!(tail[0].message, "line 3");
        assert_eq!(ring.next_seq(), 5);
    }

    /// A ring that silently discards is a ring that turns a partial record into a wrong one.
    #[test]
    fn the_log_ring_counts_what_it_dropped() {
        let mut ring = LogRing::default();
        for i in 0..(LOG_CAPACITY + 10) {
            ring.push(i as u64, 0, "firmware", format!("line {i}"));
        }
        assert_eq!(ring.len(), LOG_CAPACITY);
        assert_eq!(ring.dropped, 10);
        // Sequence numbers keep counting, so a reader can tell it missed some.
        assert_eq!(ring.next_seq(), (LOG_CAPACITY + 10) as u64);
    }
}
