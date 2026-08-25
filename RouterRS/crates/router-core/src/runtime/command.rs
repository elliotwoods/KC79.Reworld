//! Commands: the single write path into the model. GUI, OSC, and REST all
//! produce these; the model thread consumes them.

use std::sync::mpsc::Sender;

use glam::Vec2;
use router_proto::commands::ActionKind;
use router_proto::Value;

use crate::config::ImageTransmit;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    All,
    Column(usize),
    /// (column index, portal target ID)
    Portal(usize, u8),
}

pub enum Command {
    // pilot targets
    SetPilotPosition { col: usize, portal: u8, position: Vec2 },
    SetPilotPolar { col: usize, portal: u8, polar: Vec2 },
    SetPilotAxes { col: usize, portal: u8, axes: Vec2 },
    /// Cyclic set addressed by portal *vector index* (OSC /axesMove* routes).
    SetAxesCyclicByIndex { col: usize, portal_index: usize, axes: Vec2 },
    Unwind(Scope),
    ResetLocal { col: usize, portal: u8 },
    TakeCurrentPosition { col: usize, portal: u8 },

    /// The "Pilot all" drag pad: broadcast one collateable `{"m":[a,b]}` computed from a
    /// unit-circle position through the **first portal's** pilot (its offset applies to
    /// every unit — BUG-COMPAT with the C++ `Installation`/`Column` pads).
    /// `col: None` = every column.
    PilotAll { col: Option<usize>, position: Vec2 },

    // actions & polling
    PerformAction { scope: Scope, action: ActionKind },
    Poll(Scope),
    PollPosition { col: usize, portal: u8 },
    Push { col: usize, portal: u8 },
    HomeAndZeroLocal,

    // bulk hardware settings
    PushMotionProfileAll { max_velocity: i32, acceleration: Option<i32> },
    SetCurrentAll(f32),
    Broadcast { body: Value, collateable: bool },

    // installation settings
    SetTransmitMode(ImageTransmit),
    SetImageEnabled(bool),
    RebuildColumns,
    ClearOutbox(usize),

    // per-portal parameters & submodule commands (GUI inspector)
    SetPilotOffset { col: usize, portal: u8, offset: f32 },
    SetPilotSendPeriodically { col: usize, portal: u8, enabled: bool },
    SeeThroughLocal { col: usize, portal: u8 },
    SetPollRegularly { col: usize, portal: u8, enabled: bool, interval_s: f32 },
    Mc { col: usize, portal: u8, axis: usize, kind: McCommand },
    SetMotionProfile {
        col: usize,
        portal: u8,
        axis: usize,
        max_velocity: i32,
        acceleration: i32,
        min_velocity: i32,
    },
    MdTestRoutine { col: usize, portal: u8, axis: usize },
    MdTestTimer { col: usize, portal: u8, axis: usize },
    SetPortalCurrent { col: usize, portal: u8, amps: f32 },
    SetPortalMicrostep { col: usize, portal: u8, resolution: u32 },

    // column / connection management
    SetScheduledPoll { col: usize, enabled: bool, period_s: f32 },
    Rs485Connect { col: usize, settings: serde_json::Value },
    Rs485Disconnect { col: usize },
    Rs485ClearCounters { col: usize },

    // installation parameters
    SetArrangement { columns: usize, rows: usize, column_width: usize, flipped: bool },
    SetMessaging { period_s: f32, keyframe_batch_size: usize, keyframe_velocities: bool },

    // firmware update (None = all connected columns, mass parameters)
    FwUpload { col: Option<usize>, path: std::path::PathBuf },
    FwErase { col: Option<usize> },
    FwRun { col: Option<usize> },

    // image sources
    SourceAdd { type_name: String },
    SourceRemove { index: usize },
    /// Apply a JSON fragment to a source (routes through its deserialise,
    /// so any parameter key works: {"alpha": 0.5}, {"text": "HI"}, ...).
    SourceSetParams { index: usize, params: serde_json::Value },

    /// Clear one portal's firmware log ring (the C++ Logger panel's Clear button).
    ClearPortalLog { col: usize, portal: u8 },

    // misc
    Marker(String),
    SaveConfig,
    Shutdown,

    Query(Query),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McCommand {
    ZeroCurrentPosition,
    MeasureBacklash,
    HomeRoutine,
    InitTimer,
    DeinitTimer,
    TestTimer,
    PushMotionProfile,
}

pub enum Query {
    /// Live position from hardware-reported axis values.
    GetPosition {
        col: usize,
        portal: u8,
        reply: Sender<Option<Vec2>>,
    },
    GetTargetPosition {
        col: usize,
        portal: u8,
        reply: Sender<Option<Vec2>>,
    },
    IsInPosition {
        col: usize,
        portal: u8,
        reply: Sender<Option<bool>>,
    },
    /// Portal existence check (REST validation).
    PortalExists {
        col: usize,
        portal: u8,
        reply: Sender<bool>,
    },
}
