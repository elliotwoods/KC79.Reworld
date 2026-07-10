//! The application message enum. State lives in router-core; the GUI is a
//! projection, so most messages translate directly into runtime Commands.

use glam::Vec2;
use router_core::proto::commands::ActionKind;
use router_core::runtime::{McCommand, Scope};

use crate::selection::{Selection, TopModule};

#[derive(Debug, Clone)]
pub enum Message {
    /// ~60 Hz: refresh the snapshot from the runtime.
    Tick,
    Select(Selection),
    SelectCenter(TopModule),

    // broadcast / scoped operations
    Action(Scope, ActionKind),
    Poll(Scope),
    HomeAndZero,
    RebuildColumns,

    // pilot interactions (addressed at the current selection)
    PilotDragTo { col: usize, target: u8, position: Vec2 },
    PilotSetAxis { col: usize, target: u8, axis: usize, value: f32 },
    PilotOffset { col: usize, target: u8, offset: f32 },
    PilotPush,
    PilotPollPosition,
    PilotResetLocal,
    PilotUnwind,
    PilotTakeCurrent,
    PilotSeeThrough,

    // per-portal submodule commands
    Mc { axis: usize, kind: McCommand },
    MdTestRoutine { axis: usize },
    MdTestTimer { axis: usize },

    // column / connection
    ClearOutbox(usize),
    ClearCounters(usize),
    Disconnect(usize),
    ConnectSerial(usize, String),
    ConnectTcp(usize, String),
    RefreshPorts,

    // installation settings
    ToggleImageEnabled(bool),
    TransmitModeSelected(String),
    ToggleFlipped(bool),
    ToggleVelocities(bool),
    ToggleScheduledPoll(usize, bool),
    TogglePollRegularly(bool),
    ToggleSendPeriodically(bool),

    // image sources
    SourceAdd(String),
    SourceRemove(usize),
    /// Set one source parameter: (source index, key, value).
    SourceParam(usize, &'static str, serde_json::Value),
    SourceFileDialog(usize),

    // firmware update
    FwUploadDialog(Option<usize>),
    FwErase(Option<usize>),
    FwRun(Option<usize>),

    // text-input editing: (field id, new text) and submit
    Edit(&'static str, String),
    Submit(&'static str),

    // diagnostics panel
    WriteSummaryNow,
    ToggleVerbose,
    MarkerText(String),
    AddMarker,

    SaveConfig,
}
