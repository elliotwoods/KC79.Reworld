//! Read-only projection of the model published every tick for the GUI.

use glam::Vec2;

use crate::image::PixelsF32;
use crate::rs485::Rs485Stats;

#[derive(Debug, Clone, Default)]
pub struct McSnapshot {
    pub reported_position: Option<i32>,
    pub reported_target: Option<i32>,
    pub max_velocity: i32,
    pub acceleration: i32,
    pub min_velocity: i32,
    /// All four healthStatus flags true (None = unreported).
    pub health_ok: Option<bool>,
}

#[derive(Debug, Clone, Default)]
pub struct PortalSnapshot {
    pub target: u8,
    /// Local target axes (a, b).
    pub axes: Vec2,
    pub polar: Vec2,
    pub position: Vec2,
    pub live_position: Option<Vec2>,
    pub live_target_position: Option<Vec2>,
    pub live_axes: Option<Vec2>,
    pub in_target_position: bool,
    pub last_rx_age_ms: Option<u64>,
    pub last_tx_age_ms: Option<u64>,
    pub up_time_ms: Option<u64>,
    pub version: Option<String>,
    pub last_log: Option<(u8, String, u32)>,
    /// Recent firmware log lines (level, message, count), newest last.
    pub logs: Vec<(u8, String, u32)>,
    pub offset: f32,
    pub leading_control: &'static str,
    pub mc: [McSnapshot; 2],
    pub mds_current_amps: f32,
    pub mds_microstep_resolution: u32,
    pub poll_regularly: bool,
    pub poll_interval_s: f32,
    pub send_periodically: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ColumnSnapshot {
    pub index: usize,
    pub count_x: usize,
    pub count_y: usize,
    pub flipped: bool,
    pub stats: Rs485Stats,
    pub portals: Vec<PortalSnapshot>,
    pub scheduled_poll_enabled: bool,
    pub scheduled_poll_period_s: f32,
    /// The RS485 repeaters that have answered on this bus. Empty on a V1/V2
    /// installation, which has none, and on a V3 bus nobody has queried yet.
    pub repeaters: Vec<crate::model::repeater::RepeaterRecord>,
}

#[derive(Debug, Clone, Default)]
pub struct UiSnapshot {
    pub generation: u64,
    pub resolution: (usize, usize),
    pub columns: Vec<ColumnSnapshot>,
    /// Rendered image preview (tiny; cloned per snapshot).
    pub preview: PixelsF32,
    pub image_enabled: bool,
    pub transmit_mode: &'static str,
    /// (columns, rows, column width, flipped) arrangement parameters.
    pub arrangement: (usize, usize, usize, bool),
    pub period_s: f32,
    pub keyframe_batch_size: usize,
    pub keyframe_velocities: bool,
    pub osc_running: bool,
    pub osc_port: u16,
    pub rest_running: bool,
    pub rest_port: u16,
    pub osc_messages_per_tick: usize,
    /// Serialized image sources (type + parameters), for the Renderer panel.
    pub sources: Vec<serde_json::Value>,
}
