//! The bridge: the only code that touches both the bus and the runtime.
//!
//! One OS thread on a ~33 ms tick. The model thread never calls into the bus; the host's
//! 60 Hz outbound tick never touches the model. Per tick, in order:
//!
//! 1. **actions** — every `/…/actions/*` i64 counter is diffed against the last-seen value
//!    and each advance becomes a `Command` (using the current selection);
//! 2. **agent requests** — `/api/router/*` handlers queue `Command`s in [`Shared`];
//! 3. **desired diff** — writable params that changed since the bridge last saw them become
//!    `Command`s; params the *model* changed on its own are seeded back to the bus. The
//!    last-seen mirrors are updated in the same breath, so the bridge's own writes never
//!    round-trip into commands (the echo-loop guard, unit-tested below);
//! 4. **snapshot publish** — telemetry rows and change-detected observed params;
//! 5. **1 Hz** — diagnostics mirror, serial-port cache, report facts;
//! 6. **topology check** — a shape change (rebuild, arrangement, source add/remove) re-seals
//!    the schema and re-seeds everything.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use av_gui_bus::{Bus, LiveBus, ParamId, SchemaBuilder, TelemetryWriter, Value};
use glam::vec2;
use router_core::config::ImageTransmit;
use router_core::runtime::{Command, McCommand, Scope, UiSnapshot};
use router_proto::commands::ActionKind;
use router_report::{PortalState, Reporter};

use crate::schema::{self, Params, Shape};
use crate::shared::Shared;

const TICK: Duration = Duration::from_millis(33);
/// Cap on how many times one counter advance can re-fire (a reconnecting page adopts the
/// current value instead, so a large jump here is a bug, not a burst of intent).
const MAX_FIRES_PER_TICK: i64 = 4;

pub struct Bridge {
    live: Arc<LiveBus>,
    bus: Arc<Bus>,
    params: Params,
    shape: Shape,
    tx: Sender<Command>,
    snapshot_slot: Arc<Mutex<Arc<UiSnapshot>>>,
    reporter: Reporter,
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,

    actions: Vec<ActionBinding>,
    mirrors: Mirrors,
    /// Last text written per text param — `set_text` re-sends unconditionally, so the
    /// bridge change-detects here to keep TextValue traffic at change rate, not tick rate.
    texts: HashMap<ParamId, String>,
    /// (state index 0..4, score) per (col, portal), refreshed at 1 Hz from diagnostics.
    health: HashMap<(u8, u8), (u8, u8)>,
    last_generation: u64,
    last_slow: Instant,
    ticks: u64,
    tx_rx_ema: (f32, f32),
    prev_totals: (u64, u64),
    last_rates_at: Instant,
}

struct ActionBinding {
    id: ParamId,
    last: i64,
    target: ActionTarget,
}

/// The claim-once telemetry writers for one schema epoch (see [`Bridge::run`]).
struct Writers<'bus> {
    pose: Option<TelemetryWriter<'bus>>,
    link: Option<TelemetryWriter<'bus>>,
    columns: Option<TelemetryWriter<'bus>>,
    selected: Option<TelemetryWriter<'bus>>,
    osc: Option<TelemetryWriter<'bus>>,
    preview: Option<TelemetryWriter<'bus>>,
}

#[derive(Debug, Clone)]
enum ActionTarget {
    Installation(String),
    Bulk(String),
    Report(String),
    Column(usize, String),
    Portal(String),
    PortalAxis(usize, String),
    SourcesAdd(String),
    Source(usize, String),
}

/// Last-seen values for every writable param the bridge syncs, plus the model side.
#[derive(Default)]
struct Mirrors {
    installation: InstallationState,
    installation_model: InstallationState,
    pilot_all: [f32; 2],
    columns: Vec<ColumnState>,
    columns_model: Vec<ColumnState>,
    column_pads: Vec<[f32; 2]>,
    portal: PortalDesired,
    portal_model: PortalDesired,
    /// The (col, portal target) the proxy currently mirrors.
    selected: (i32, i32),
    sources: Vec<SourceState>,
    sources_model: Vec<SourceState>,
    verbose: bool,
    observed: ObservedState,
}

#[derive(Default, Clone, PartialEq)]
struct InstallationState {
    columns: i32,
    rows: i32,
    column_width: i32,
    /// Rows per panel; 0 when a column is one flat grid rather than a stack of panels.
    panel_height: i32,
    flipped: bool,
    transmit: u32,
    period_s: f32,
    keyframe_batch: i32,
    keyframe_velocities: bool,
    image_enabled: bool,
}

#[derive(Default, Clone, PartialEq)]
struct ColumnState {
    scheduled_poll_enabled: bool,
    scheduled_poll_period_s: f32,
}

#[derive(Default, Clone, PartialEq)]
struct PortalDesired {
    position: [f32; 2],
    polar: [f32; 2],
    axes: [f32; 2],
    offset: f32,
    send_periodically: bool,
    poll_regularly: bool,
    poll_interval_s: f32,
    mds_current: f32,
    mds_microstep_idx: u32,
    profiles: [[i32; 3]; 2],
}

/// One source's writable params as a JSON patch document (leaf -> serialized value).
#[derive(Default, Clone, PartialEq)]
struct SourceState(serde_json::Map<String, serde_json::Value>);

#[derive(Default, Clone, PartialEq)]
struct ObservedState {
    osc_running: bool,
    osc_port: i32,
    rest_running: bool,
    rest_port: i32,
    session_file: String,
    file_bytes: i64,
    dropped: i64,
    faulty: i32,
}

// ------------------------------------------------------------------ value helpers

fn f32_of(v: Option<Value>) -> f32 {
    match v {
        Some(Value::F32(x)) => x,
        _ => 0.0,
    }
}
fn i32_of(v: Option<Value>) -> i32 {
    match v {
        Some(Value::I32(x)) => x,
        _ => 0,
    }
}
fn i64_of(v: Option<Value>) -> i64 {
    match v {
        Some(Value::I64(x)) => x,
        _ => 0,
    }
}
fn bool_of(v: Option<Value>) -> bool {
    matches!(v, Some(Value::Bool(true)))
}
fn enum_of(v: Option<Value>) -> u32 {
    match v {
        Some(Value::Enum(x)) => x,
        _ => 0,
    }
}
fn vec2_of(v: Option<Value>) -> [f32; 2] {
    match v {
        Some(Value::Vec2(x)) => x,
        _ => [0.0, 0.0],
    }
}

fn leading_index(name: &str) -> u32 {
    match name {
        "Position" => 0,
        "Polar" => 1,
        _ => 2,
    }
}

fn portal_state_index(state: PortalState) -> u8 {
    match state {
        PortalState::Unknown => 0,
        PortalState::Ok => 1,
        PortalState::Degraded => 2,
        PortalState::Faulty => 3,
        PortalState::Silent => 4,
    }
}

fn action_kind(leaf: &str) -> Option<ActionKind> {
    Some(match leaf {
        "ping" => ActionKind::Ping,
        "init" => ActionKind::Init,
        "calibrate" => ActionKind::Calibrate,
        "home" => ActionKind::Home,
        "flash_leds" => ActionKind::FlashLeds,
        "go_home" => ActionKind::GoHome,
        "see_through" => ActionKind::SeeThrough,
        "lights_off" => ActionKind::DisableDebugLights,
        "lights_on" => ActionKind::EnableDebugLights,
        "unjam" => ActionKind::Unjam,
        "escape" => ActionKind::EscapeFromRoutine,
        "reboot" => ActionKind::Reboot,
        _ => return None,
    })
}

/// Schema leaf -> the JSON key the source `deserialise` expects.
fn source_json_key(leaf: &str) -> &str {
    match leaf {
        "render_enabled" => "renderEnabled",
        "gradient_type" => "gradientType",
        "loop_mode" => "loopMode",
        "sender_name" => "senderName",
        other => other,
    }
}

/// Enum leaf -> its wire-string table (index order matches the schema declaration).
fn source_enum_string(leaf: &str, index: u32) -> Option<&'static str> {
    let table: &[&str] = match leaf {
        "style" => &["Direct", "HV_ThetaR", "Centered"],
        "gradient_type" => &["Radial", "Horizontal", "Vertical"],
        "wave" => &["Triangle", "Sine", "Sawtooth"],
        "loop_mode" => &["Loop", "Ping Pong", "None"],
        _ => return None,
    };
    table.get(index as usize).copied()
}

impl Bridge {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        live: Arc<LiveBus>,
        params: Params,
        shape: Shape,
        tx: Sender<Command>,
        snapshot_slot: Arc<Mutex<Arc<UiSnapshot>>>,
        reporter: Reporter,
        shared: Arc<Shared>,
        stop: Arc<AtomicBool>,
        initial_devices: Vec<Option<serde_json::Value>>,
    ) -> Self {
        let bus = live.current();
        let mut bridge = Self {
            live,
            bus,
            params,
            shape,
            tx,
            snapshot_slot,
            reporter,
            shared,
            stop,
            actions: Vec::new(),
            mirrors: Mirrors::default(),
            texts: HashMap::new(),
            health: HashMap::new(),
            last_generation: 0,
            last_slow: Instant::now() - Duration::from_secs(10),
            ticks: 0,
            tx_rx_ema: (0.0, 0.0),
            prev_totals: (0, 0),
            last_rates_at: Instant::now(),
        };
        bridge.rebind();
        // Seed the per-column device settings from the loaded config so the picker shows
        // what the column will connect to.
        for (i, device) in initial_devices.into_iter().enumerate() {
            if let (Some(device), Some(column)) = (device, bridge.params.columns.get(i)) {
                let _ = bridge.bus.set_text(column.device, &device.to_string());
            }
        }
        bridge
    }

    pub fn run(mut self) {
        // Telemetry writers are claim-once per ring per bus: acquire them once per schema
        // epoch and hold them for the whole inner loop. A re-seal builds new rings, so the
        // inner loop breaks and the next outer iteration re-acquires from the new bus —
        // exactly the "stranded writer" hazard `CarryReport::writers_to_reacquire` warns
        // about, handled structurally.
        while !self.stop.load(Ordering::Acquire) {
            let bus = self.bus.clone();
            let writers = Writers {
                pose: bus.telemetry_writer(self.params.tel_pose),
                link: bus.telemetry_writer(self.params.tel_link),
                columns: bus.telemetry_writer(self.params.tel_columns),
                selected: bus.telemetry_writer(self.params.tel_selected),
                osc: bus.telemetry_writer(self.params.tel_osc),
                preview: bus.telemetry_writer(self.params.tel_preview),
            };
            let epoch = self.live.epoch();
            while !self.stop.load(Ordering::Acquire) && self.live.epoch() == epoch {
                let started = Instant::now();
                self.tick(&writers);
                self.ticks += 1;
                std::thread::sleep(TICK.saturating_sub(started.elapsed()));
            }
        }
    }

    fn tick(&mut self, writers: &Writers<'_>) {
        let snap = self.snapshot_slot.lock().unwrap().clone();

        self.fire_actions(&snap);
        self.drain_requests();
        self.sync_installation(&snap);
        self.sync_columns(&snap);
        self.sync_portal_proxy(&snap);
        self.sync_sources(&snap);
        self.sync_verbose();

        if snap.generation != self.last_generation {
            self.publish_telemetry(&snap, writers);
            self.publish_observed(&snap);
            *self.shared.snapshot.lock().unwrap() = snap.clone();
            self.last_generation = snap.generation;
        }

        if self.last_slow.elapsed() >= Duration::from_secs(1) {
            self.slow_jobs(&snap);
            self.last_slow = Instant::now();
        }

        self.check_shape(&snap);
    }

    // -------------------------------------------------------------------- setup / reseal

    /// (Re)build the action bindings and seed every mirror from the current bus values.
    /// Called at start and after every re-seal; nothing fires from the seed itself.
    fn rebind(&mut self) {
        self.actions.clear();
        for decl in self.bus.schema().params() {
            let path = decl.path.as_str();
            let Some((prefix, leaf)) = path.split_once("/actions/") else {
                continue;
            };
            let target = if prefix == "/installation" {
                ActionTarget::Installation(leaf.into())
            } else if prefix == "/bulk" {
                ActionTarget::Bulk(leaf.into())
            } else if prefix == "/report" {
                ActionTarget::Report(leaf.into())
            } else if prefix == "/portal" {
                ActionTarget::Portal(leaf.into())
            } else if prefix == "/portal/log" {
                ActionTarget::Portal(format!("log_{leaf}"))
            } else if prefix == "/portal/axis/a" {
                ActionTarget::PortalAxis(0, leaf.into())
            } else if prefix == "/portal/axis/b" {
                ActionTarget::PortalAxis(1, leaf.into())
            } else if prefix == "/sources" {
                ActionTarget::SourcesAdd(leaf.into())
            } else if let Some(rest) = prefix.strip_prefix("/columns/") {
                match rest.parse::<usize>() {
                    Ok(col) => ActionTarget::Column(col, leaf.into()),
                    Err(_) => continue,
                }
            } else if let Some(rest) = prefix.strip_prefix("/sources/") {
                match rest.parse::<usize>() {
                    Ok(index) => ActionTarget::Source(index, leaf.into()),
                    Err(_) => continue,
                }
            } else {
                continue;
            };
            let Some(id) = self.bus.id_of(path) else {
                continue;
            };
            self.actions.push(ActionBinding {
                id,
                last: i64_of(self.bus.get(id)),
                target,
            });
        }

        self.texts.clear();
        self.mirrors = Mirrors::default();
        self.mirrors.installation = self.read_installation_bus();
        self.mirrors.installation_model = self.mirrors.installation.clone();
        self.mirrors.pilot_all = vec2_of(self.bus.get(self.params.installation_pilot_all));
        self.mirrors.columns = (0..self.params.columns.len())
            .map(|i| self.read_column_bus(i))
            .collect();
        self.mirrors.columns_model = self.mirrors.columns.clone();
        self.mirrors.column_pads = vec![[0.0, 0.0]; self.params.columns.len()];
        self.mirrors.portal = self.read_portal_bus();
        self.mirrors.portal_model = self.mirrors.portal.clone();
        self.mirrors.selected = (
            i32_of(self.bus.get(self.params.select_col)),
            i32_of(self.bus.get(self.params.select_portal)),
        );
        self.mirrors.sources = (0..self.params.sources.len())
            .map(|i| self.read_source_bus(i))
            .collect();
        self.mirrors.sources_model = self.mirrors.sources.clone();
        self.mirrors.verbose = bool_of(self.bus.get(self.params.report_verbose));
        self.last_generation = 0; // force a republish on the new epoch
    }

    fn check_shape(&mut self, snap: &UiSnapshot) {
        if snap.generation == 0 {
            return;
        }
        let live_shape = Shape {
            columns: snap
                .columns
                .iter()
                .map(|c| (c.count_x, c.count_y))
                .collect(),
            resolution: (snap.preview.width, snap.preview.height),
            sources: snap
                .sources
                .iter()
                .filter_map(|s| s.get("type").and_then(|t| t.as_str()).map(String::from))
                .collect(),
        };
        if live_shape == self.shape || live_shape.columns.is_empty() {
            return;
        }
        tracing::info!(
            "installation shape changed ({} columns, {} sources) — re-sealing the schema",
            live_shape.columns.len(),
            live_shape.sources.len()
        );
        let mut builder = SchemaBuilder::new();
        let simulated = bool_of(self.bus.get(self.params.app_simulated));
        if let Err(error) = schema::declare(&mut builder, &live_shape, simulated) {
            tracing::error!("schema declare failed: {error}");
            return;
        }
        // Carry setup facts by hand: reseal carries values by path, but re-resolve + reseed
        // is what makes the new subtrees correct.
        match self.live.reseal(builder) {
            Ok(_report) => {
                self.bus = self.live.current();
                match Params::resolve(&self.bus, &live_shape) {
                    Ok(params) => {
                        self.params = params;
                        self.shape = live_shape;
                        self.rebind();
                    }
                    Err(error) => tracing::error!("param resolve after reseal failed: {error}"),
                }
            }
            Err(error) => tracing::error!("schema reseal failed: {error:?}"),
        }
    }

    // -------------------------------------------------------------------- actions

    fn selection(&self) -> (usize, u8) {
        let col = i32_of(self.bus.get(self.params.select_col)).max(0) as usize;
        let portal = i32_of(self.bus.get(self.params.select_portal)).clamp(0, 255) as u8;
        (col, portal)
    }

    fn fire_actions(&mut self, snap: &UiSnapshot) {
        let mut fired: Vec<(ActionTarget, i64)> = Vec::new();
        for binding in &mut self.actions {
            let value = i64_of(self.bus.get(binding.id));
            if value > binding.last {
                fired.push((
                    binding.target.clone(),
                    (value - binding.last).min(MAX_FIRES_PER_TICK),
                ));
            }
            binding.last = value;
        }
        for (target, times) in fired {
            for _ in 0..times {
                self.fire(&target, snap);
            }
        }
    }

    fn fire(&mut self, target: &ActionTarget, snap: &UiSnapshot) {
        let send = |command: Command| {
            let _ = self.tx.send(command);
        };
        match target {
            ActionTarget::Installation(leaf) => match leaf.as_str() {
                "poll" => send(Command::Poll(Scope::All)),
                "home_and_zero" => send(Command::HomeAndZeroLocal),
                "rebuild_columns" => send(Command::RebuildColumns),
                "save_config" => send(Command::SaveConfig),
                other => {
                    if let Some(kind) = action_kind(other) {
                        send(Command::PerformAction {
                            scope: Scope::All,
                            action: kind,
                        });
                    }
                }
            },
            ActionTarget::Bulk(leaf) => match leaf.as_str() {
                "push_motion_profile" => send(Command::PushMotionProfileAll {
                    max_velocity: i32_of(self.bus.get(self.params.bulk_max_velocity)),
                    acceleration: Some(i32_of(self.bus.get(self.params.bulk_acceleration))),
                }),
                "set_current" => send(Command::SetCurrentAll(f32_of(
                    self.bus.get(self.params.bulk_current_amps),
                ))),
                _ => {}
            },
            ActionTarget::Report(leaf) => match leaf.as_str() {
                "mark" => {
                    let text = self
                        .bus
                        .text(self.params.report_marker_text, |s| s.to_string())
                        .unwrap_or_default();
                    send(Command::Marker(text));
                }
                "write_summary" => {
                    self.reporter.write_summary_now();
                }
                _ => {}
            },
            ActionTarget::Column(col, leaf) => match leaf.as_str() {
                "poll" => send(Command::Poll(Scope::Column(*col))),
                "connect" => {
                    let text = self
                        .params
                        .columns
                        .get(*col)
                        .and_then(|c| self.bus.text(c.device, |s| s.to_string()).ok())
                        .unwrap_or_default();
                    match serde_json::from_str::<serde_json::Value>(&text) {
                        Ok(settings) if settings.is_object() => send(Command::Rs485Connect {
                            col: *col,
                            settings,
                        }),
                        _ => tracing::warn!("column {col}: device settings are not valid JSON"),
                    }
                }
                "disconnect" => send(Command::Rs485Disconnect { col: *col }),
                "clear_outbox" => send(Command::ClearOutbox(*col)),
                "clear_counters" => send(Command::Rs485ClearCounters { col: *col }),
                other => {
                    if let Some(kind) = action_kind(other) {
                        send(Command::PerformAction {
                            scope: Scope::Column(*col),
                            action: kind,
                        });
                    }
                }
            },
            ActionTarget::Portal(leaf) => {
                let (col, portal) = self.selection();
                match leaf.as_str() {
                    "poll" => send(Command::Poll(Scope::Portal(col, portal))),
                    "reset_local" => send(Command::ResetLocal { col, portal }),
                    "unwind" => send(Command::Unwind(Scope::Portal(col, portal))),
                    "push" => send(Command::Push { col, portal }),
                    "poll_position" => send(Command::PollPosition { col, portal }),
                    "take_current" => send(Command::TakeCurrentPosition { col, portal }),
                    "see_through_local" => send(Command::SeeThroughLocal { col, portal }),
                    "log_clear" => send(Command::ClearPortalLog { col, portal }),
                    other => {
                        if let Some(kind) = action_kind(other) {
                            send(Command::PerformAction {
                                scope: Scope::Portal(col, portal),
                                action: kind,
                            });
                        }
                    }
                }
            }
            ActionTarget::PortalAxis(axis, leaf) => {
                let (col, portal) = self.selection();
                let axis = *axis;
                let mc = |kind: McCommand| Command::Mc {
                    col,
                    portal,
                    axis,
                    kind,
                };
                match leaf.as_str() {
                    "zero_position" => send(mc(McCommand::ZeroCurrentPosition)),
                    "measure_backlash" => send(mc(McCommand::MeasureBacklash)),
                    "home_routine" => send(mc(McCommand::HomeRoutine)),
                    "init_timer" => send(mc(McCommand::InitTimer)),
                    "deinit_timer" => send(mc(McCommand::DeinitTimer)),
                    "test_timer" => send(mc(McCommand::TestTimer)),
                    "push_profile" => send(mc(McCommand::PushMotionProfile)),
                    "md_test_routine" => send(Command::MdTestRoutine { col, portal, axis }),
                    "md_test_timer" => send(Command::MdTestTimer { col, portal, axis }),
                    _ => {}
                }
            }
            ActionTarget::SourcesAdd(leaf) => {
                let type_name = match leaf.as_str() {
                    "add_gradient" => "Gradient",
                    "add_text" => "Text",
                    "add_file_player" => "FilePlayer",
                    "add_spout" => "Spout",
                    _ => return,
                };
                send(Command::SourceAdd {
                    type_name: type_name.into(),
                });
            }
            ActionTarget::Source(index, leaf) => match leaf.as_str() {
                "remove" => send(Command::SourceRemove { index: *index }),
                "clear_file" => send(Command::SourceSetParams {
                    index: *index,
                    params: serde_json::json!({ "file": "" }),
                }),
                "jump_to_start" => send(Command::SourceSetParams {
                    index: *index,
                    params: serde_json::json!({ "position": 0.0 }),
                }),
                _ => {}
            },
        }
        let _ = snap;
    }

    fn drain_requests(&mut self) {
        let requests: Vec<Command> = std::mem::take(&mut *self.shared.requests.lock().unwrap());
        for command in requests {
            let _ = self.tx.send(command);
        }
    }

    // -------------------------------------------------------------------- desired diffs

    fn read_installation_bus(&self) -> InstallationState {
        let p = &self.params;
        InstallationState {
            columns: i32_of(self.bus.get(p.arrangement_columns)),
            rows: i32_of(self.bus.get(p.arrangement_rows)),
            column_width: i32_of(self.bus.get(p.arrangement_column_width)),
            panel_height: i32_of(self.bus.get(p.arrangement_panel_height)),
            flipped: bool_of(self.bus.get(p.arrangement_flipped)),
            transmit: enum_of(self.bus.get(p.messaging_transmit)),
            period_s: f32_of(self.bus.get(p.messaging_period_s)),
            keyframe_batch: i32_of(self.bus.get(p.messaging_keyframe_batch)),
            keyframe_velocities: bool_of(self.bus.get(p.messaging_keyframe_velocities)),
            image_enabled: bool_of(self.bus.get(p.image_enabled)),
        }
    }

    fn installation_from_model(snap: &UiSnapshot) -> InstallationState {
        InstallationState {
            columns: snap.arrangement.0 as i32,
            rows: snap.arrangement.1 as i32,
            column_width: snap.arrangement.2 as i32,
            panel_height: snap.arrangement.3 as i32,
            flipped: snap.arrangement.4,
            transmit: match snap.transmit_mode {
                "Keyframe" => 1,
                "Disabled" => 2,
                _ => 0,
            },
            period_s: snap.period_s,
            keyframe_batch: snap.keyframe_batch_size as i32,
            keyframe_velocities: snap.keyframe_velocities,
            image_enabled: snap.image_enabled,
        }
    }

    fn sync_installation(&mut self, snap: &UiSnapshot) {
        let p = &self.params;
        let bus_now = self.read_installation_bus();
        let last = self.mirrors.installation.clone();
        if bus_now != last {
            // The client changed something: forward precisely what moved.
            let b = &bus_now;
            if (b.columns, b.rows, b.column_width, b.flipped)
                != (last.columns, last.rows, last.column_width, last.flipped)
            {
                let _ = self.tx.send(Command::SetArrangement {
                    columns: b.columns.max(0) as usize,
                    rows: b.rows.max(0) as usize,
                    column_width: b.column_width.max(0) as usize,
                    flipped: b.flipped,
                });
            }
            if b.transmit != last.transmit {
                let mode = match b.transmit {
                    1 => ImageTransmit::Keyframe,
                    2 => ImageTransmit::Disabled,
                    _ => ImageTransmit::Individual,
                };
                let _ = self.tx.send(Command::SetTransmitMode(mode));
            }
            if (b.period_s, b.keyframe_batch, b.keyframe_velocities)
                != (last.period_s, last.keyframe_batch, last.keyframe_velocities)
            {
                let _ = self.tx.send(Command::SetMessaging {
                    period_s: b.period_s,
                    keyframe_batch_size: b.keyframe_batch.max(0) as usize,
                    keyframe_velocities: b.keyframe_velocities,
                });
            }
            if b.image_enabled != last.image_enabled {
                let _ = self.tx.send(Command::SetImageEnabled(b.image_enabled));
            }
            self.mirrors.installation = bus_now;
        } else {
            let model_now = Self::installation_from_model(snap);
            if model_now != self.mirrors.installation_model && snap.generation != 0 {
                let _ = self
                    .bus
                    .set(p.arrangement_columns, Value::I32(model_now.columns));
                let _ = self.bus.set(p.arrangement_rows, Value::I32(model_now.rows));
                let _ = self.bus.set(
                    p.arrangement_column_width,
                    Value::I32(model_now.column_width),
                );
                let _ = self.bus.set(
                    p.arrangement_panel_height,
                    Value::I32(model_now.panel_height),
                );
                let _ = self
                    .bus
                    .set(p.arrangement_flipped, Value::Bool(model_now.flipped));
                let _ = self
                    .bus
                    .set(p.messaging_transmit, Value::Enum(model_now.transmit));
                let _ = self
                    .bus
                    .set(p.messaging_period_s, Value::F32(model_now.period_s));
                let _ = self.bus.set(
                    p.messaging_keyframe_batch,
                    Value::I32(model_now.keyframe_batch),
                );
                let _ = self.bus.set(
                    p.messaging_keyframe_velocities,
                    Value::Bool(model_now.keyframe_velocities),
                );
                let _ = self
                    .bus
                    .set(p.image_enabled, Value::Bool(model_now.image_enabled));
                self.mirrors.installation = model_now.clone();
                self.mirrors.installation_model = model_now;
            }
        }

        // The pilot-all pad: pure control, no model echo. Stream while it moves.
        let pad = vec2_of(self.bus.get(self.params.installation_pilot_all));
        if pad != self.mirrors.pilot_all {
            let _ = self.tx.send(Command::PilotAll {
                col: None,
                position: vec2(pad[0], pad[1]),
            });
            self.mirrors.pilot_all = pad;
        }
    }

    fn read_column_bus(&self, i: usize) -> ColumnState {
        let Some(c) = self.params.columns.get(i) else {
            return ColumnState::default();
        };
        ColumnState {
            scheduled_poll_enabled: bool_of(self.bus.get(c.scheduled_poll_enabled)),
            scheduled_poll_period_s: f32_of(self.bus.get(c.scheduled_poll_period_s)),
        }
    }

    fn sync_columns(&mut self, snap: &UiSnapshot) {
        for i in 0..self.params.columns.len() {
            let bus_now = self.read_column_bus(i);
            let last = self.mirrors.columns.get(i).cloned().unwrap_or_default();
            if bus_now != last {
                let _ = self.tx.send(Command::SetScheduledPoll {
                    col: i,
                    enabled: bus_now.scheduled_poll_enabled,
                    period_s: bus_now.scheduled_poll_period_s,
                });
                if let Some(slot) = self.mirrors.columns.get_mut(i) {
                    *slot = bus_now;
                }
            } else if let Some(column) = snap.columns.get(i) {
                let model_now = ColumnState {
                    scheduled_poll_enabled: column.scheduled_poll_enabled,
                    scheduled_poll_period_s: column.scheduled_poll_period_s,
                };
                if Some(&model_now) != self.mirrors.columns_model.get(i) {
                    if let Some(c) = self.params.columns.get(i) {
                        let _ = self.bus.set(
                            c.scheduled_poll_enabled,
                            Value::Bool(model_now.scheduled_poll_enabled),
                        );
                        let _ = self.bus.set(
                            c.scheduled_poll_period_s,
                            Value::F32(model_now.scheduled_poll_period_s),
                        );
                    }
                    if let Some(slot) = self.mirrors.columns.get_mut(i) {
                        *slot = model_now.clone();
                    }
                    if let Some(slot) = self.mirrors.columns_model.get_mut(i) {
                        *slot = model_now;
                    }
                }
            }

            // Column-scoped pilot-all pad.
            if let Some(c) = self.params.columns.get(i) {
                let pad = vec2_of(self.bus.get(c.pilot_all));
                if Some(&pad) != self.mirrors.column_pads.get(i) {
                    let _ = self.tx.send(Command::PilotAll {
                        col: Some(i),
                        position: vec2(pad[0], pad[1]),
                    });
                    if let Some(slot) = self.mirrors.column_pads.get_mut(i) {
                        *slot = pad;
                    }
                }
            }
        }
    }

    fn read_portal_bus(&self) -> PortalDesired {
        let p = &self.params.portal;
        PortalDesired {
            position: vec2_of(self.bus.get(p.position)),
            polar: vec2_of(self.bus.get(p.polar)),
            axes: vec2_of(self.bus.get(p.axes)),
            offset: f32_of(self.bus.get(p.offset)),
            send_periodically: bool_of(self.bus.get(p.send_periodically)),
            poll_regularly: bool_of(self.bus.get(p.poll_regularly)),
            poll_interval_s: f32_of(self.bus.get(p.poll_interval_s)),
            mds_current: f32_of(self.bus.get(p.mds_current_amps)),
            mds_microstep_idx: enum_of(self.bus.get(p.mds_microstep_resolution)),
            profiles: [
                [
                    i32_of(self.bus.get(p.axis[0].max_velocity)),
                    i32_of(self.bus.get(p.axis[0].acceleration)),
                    i32_of(self.bus.get(p.axis[0].min_velocity)),
                ],
                [
                    i32_of(self.bus.get(p.axis[1].max_velocity)),
                    i32_of(self.bus.get(p.axis[1].acceleration)),
                    i32_of(self.bus.get(p.axis[1].min_velocity)),
                ],
            ],
        }
    }

    fn portal_from_model(snap: &UiSnapshot, col: usize, target: u8) -> Option<PortalDesired> {
        let portal = snap
            .columns
            .get(col)?
            .portals
            .iter()
            .find(|p| p.target == target)?;
        Some(PortalDesired {
            position: [portal.position.x, portal.position.y],
            polar: [portal.polar.x, portal.polar.y],
            axes: [portal.axes.x, portal.axes.y],
            offset: portal.offset,
            send_periodically: portal.send_periodically,
            poll_regularly: portal.poll_regularly,
            poll_interval_s: portal.poll_interval_s,
            mds_current: portal.mds_current_amps,
            mds_microstep_idx: portal.mds_microstep_resolution.max(1).trailing_zeros(),
            profiles: [
                [
                    portal.mc[0].max_velocity,
                    portal.mc[0].acceleration,
                    portal.mc[0].min_velocity,
                ],
                [
                    portal.mc[1].max_velocity,
                    portal.mc[1].acceleration,
                    portal.mc[1].min_velocity,
                ],
            ],
        })
    }

    fn seed_portal_proxy(&mut self, model: &PortalDesired) {
        let p = &self.params.portal;
        let _ = self.bus.set(p.position, Value::Vec2(model.position));
        let _ = self.bus.set(p.polar, Value::Vec2(model.polar));
        let _ = self.bus.set(p.axes, Value::Vec2(model.axes));
        let _ = self.bus.set(p.offset, Value::F32(model.offset));
        let _ = self
            .bus
            .set(p.send_periodically, Value::Bool(model.send_periodically));
        let _ = self
            .bus
            .set(p.poll_regularly, Value::Bool(model.poll_regularly));
        let _ = self
            .bus
            .set(p.poll_interval_s, Value::F32(model.poll_interval_s));
        let _ = self
            .bus
            .set(p.mds_current_amps, Value::F32(model.mds_current));
        let _ = self.bus.set(
            p.mds_microstep_resolution,
            Value::Enum(model.mds_microstep_idx),
        );
        for axis in 0..2 {
            let _ = self.bus.set(
                p.axis[axis].max_velocity,
                Value::I32(model.profiles[axis][0]),
            );
            let _ = self.bus.set(
                p.axis[axis].acceleration,
                Value::I32(model.profiles[axis][1]),
            );
            let _ = self.bus.set(
                p.axis[axis].min_velocity,
                Value::I32(model.profiles[axis][2]),
            );
        }
        self.mirrors.portal = model.clone();
        self.mirrors.portal_model = model.clone();
    }

    fn sync_portal_proxy(&mut self, snap: &UiSnapshot) {
        let selected = (
            i32_of(self.bus.get(self.params.select_col)),
            i32_of(self.bus.get(self.params.select_portal)),
        );
        let (col, target) = (selected.0.max(0) as usize, selected.1.clamp(0, 255) as u8);
        let model = Self::portal_from_model(snap, col, target);

        if selected != self.mirrors.selected {
            // Selection moved: repoint the proxy, seed everything, no commands.
            self.mirrors.selected = selected;
            if let Some(model) = &model {
                self.seed_portal_proxy(model);
            }
            self.publish_portal_observed(snap, col, target);
            return;
        }

        let Some(model) = model else {
            let _ = self.bus.set(self.params.portal.exists, Value::Bool(false));
            return;
        };

        let bus_now = self.read_portal_bus();
        let last = self.mirrors.portal.clone();
        if bus_now != last {
            let p = &self.params.portal;
            let _ = p; // ids only needed below through self.params
            if bus_now.position != last.position {
                let _ = self.tx.send(Command::SetPilotPosition {
                    col,
                    portal: target,
                    position: vec2(bus_now.position[0], bus_now.position[1]),
                });
            }
            if bus_now.polar != last.polar {
                let _ = self.tx.send(Command::SetPilotPolar {
                    col,
                    portal: target,
                    polar: vec2(bus_now.polar[0], bus_now.polar[1]),
                });
            }
            if bus_now.axes != last.axes {
                let _ = self.tx.send(Command::SetPilotAxes {
                    col,
                    portal: target,
                    axes: vec2(bus_now.axes[0], bus_now.axes[1]),
                });
            }
            if bus_now.offset != last.offset {
                let _ = self.tx.send(Command::SetPilotOffset {
                    col,
                    portal: target,
                    offset: bus_now.offset,
                });
            }
            if bus_now.send_periodically != last.send_periodically {
                let _ = self.tx.send(Command::SetPilotSendPeriodically {
                    col,
                    portal: target,
                    enabled: bus_now.send_periodically,
                });
            }
            if (bus_now.poll_regularly, bus_now.poll_interval_s)
                != (last.poll_regularly, last.poll_interval_s)
            {
                let _ = self.tx.send(Command::SetPollRegularly {
                    col,
                    portal: target,
                    enabled: bus_now.poll_regularly,
                    interval_s: bus_now.poll_interval_s,
                });
            }
            if bus_now.mds_current != last.mds_current {
                let _ = self.tx.send(Command::SetPortalCurrent {
                    col,
                    portal: target,
                    amps: bus_now.mds_current,
                });
            }
            if bus_now.mds_microstep_idx != last.mds_microstep_idx {
                let _ = self.tx.send(Command::SetPortalMicrostep {
                    col,
                    portal: target,
                    resolution: 1u32 << bus_now.mds_microstep_idx,
                });
            }
            for axis in 0..2 {
                if bus_now.profiles[axis] != last.profiles[axis] {
                    let _ = self.tx.send(Command::SetMotionProfile {
                        col,
                        portal: target,
                        axis,
                        max_velocity: bus_now.profiles[axis][0],
                        acceleration: bus_now.profiles[axis][1],
                        min_velocity: bus_now.profiles[axis][2],
                    });
                }
            }
            self.mirrors.portal = bus_now;
        } else if model != self.mirrors.portal_model {
            self.seed_portal_proxy(&model);
        }

        self.publish_portal_observed(snap, col, target);
    }

    fn publish_portal_observed(&mut self, snap: &UiSnapshot, col: usize, target: u8) {
        let exists_id = self.params.portal.exists;
        let portal = snap
            .columns
            .get(col)
            .and_then(|c| c.portals.iter().find(|p| p.target == target));
        let _ = self.bus.set(exists_id, Value::Bool(portal.is_some()));
        let Some(portal) = portal else { return };
        let p = &self.params.portal;
        let _ = self
            .bus
            .set(p.target_id, Value::I32(i32::from(portal.target)));
        let _ = self.bus.set(
            p.leading,
            Value::Enum(leading_index(portal.leading_control)),
        );
        let _ = self.bus.set(
            p.uptime_ms,
            Value::I64(portal.up_time_ms.unwrap_or(0) as i64),
        );
        let _ = self
            .bus
            .set(p.in_position, Value::Bool(portal.in_target_position));
        let version = portal.version.clone().unwrap_or_default();
        let (log_text, log_level) = match &portal.last_log {
            Some((level, message, count)) if *count > 1 => {
                (format!("{message} ×{count}"), i32::from(*level))
            }
            Some((level, message, _)) => (message.clone(), i32::from(*level)),
            None => (String::new(), 0),
        };
        for axis in 0..2 {
            let mc = &portal.mc[axis];
            let a = &p.axis[axis];
            let _ = self.bus.set(
                a.reported_position,
                Value::I32(mc.reported_position.unwrap_or(0)),
            );
            let _ = self.bus.set(
                a.reported_target,
                Value::I32(mc.reported_target.unwrap_or(0)),
            );
            let health = match mc.health_ok {
                None => -1,
                Some(false) => 0,
                Some(true) => 1,
            };
            let _ = self.bus.set(a.health_ok, Value::I32(health));
        }
        let (version_id, last_log_id, level_id) = (p.version, p.last_log, p.last_log_level);
        self.set_text_cached(version_id, &version);
        self.set_text_cached(last_log_id, &log_text);
        let _ = self.bus.set(level_id, Value::I32(log_level));
    }

    // -------------------------------------------------------------------- sources

    /// Serialize one source's *writable* params from the bus into a JSON patch document.
    fn read_source_bus(&self, i: usize) -> SourceState {
        let mut doc = serde_json::Map::new();
        let Some(source) = self.params.sources.get(i) else {
            return SourceState(doc);
        };
        doc.insert(
            "visible".into(),
            bool_of(self.bus.get(source.visible)).into(),
        );
        doc.insert(
            "renderEnabled".into(),
            bool_of(self.bus.get(source.render_enabled)).into(),
        );
        doc.insert("alpha".into(), f32_of(self.bus.get(source.alpha)).into());
        if let Some(style) = source_enum_string("style", enum_of(self.bus.get(source.style))) {
            doc.insert("style".into(), style.into());
        }
        for (leaf, id) in &source.extras {
            let key = source_json_key(leaf).to_string();
            let value: serde_json::Value = if let Some(text) =
                (matches!(leaf.as_str(), "text" | "font" | "sender_name" | "file"))
                    .then(|| self.bus.text(*id, |s| s.to_string()).ok())
                    .flatten()
            {
                text.into()
            } else if let Some(wire) = source_enum_string(leaf, enum_of(self.bus.get(*id))) {
                wire.into()
            } else {
                match self.bus.get(*id) {
                    Some(Value::Bool(b)) => b.into(),
                    Some(Value::I32(n)) => n.into(),
                    Some(Value::F32(f)) => f.into(),
                    Some(Value::Vec2([x, y])) => serde_json::json!([x, y]),
                    _ => continue,
                }
            };
            doc.insert(key, value);
        }
        SourceState(doc)
    }

    /// The same document, read from the model's serialized source.
    fn source_from_model(source: &serde_json::Value, params: &schema::SourceParams) -> SourceState {
        let mut doc = serde_json::Map::new();
        let mut copy = |key: &str| {
            if let Some(v) = source.get(key) {
                doc.insert(key.to_string(), v.clone());
            }
        };
        copy("visible");
        copy("renderEnabled");
        copy("alpha");
        copy("style");
        for (leaf, _) in &params.extras {
            copy(source_json_key(leaf));
        }
        SourceState(doc)
    }

    fn sync_sources(&mut self, snap: &UiSnapshot) {
        for i in 0..self.params.sources.len() {
            let bus_now = self.read_source_bus(i);
            let last = self.mirrors.sources.get(i).cloned().unwrap_or_default();
            if bus_now != last {
                // Forward only the changed keys.
                let mut patch = serde_json::Map::new();
                for (key, value) in &bus_now.0 {
                    if last.0.get(key) != Some(value) {
                        patch.insert(key.clone(), value.clone());
                    }
                }
                if !patch.is_empty() {
                    let _ = self.tx.send(Command::SourceSetParams {
                        index: i,
                        params: serde_json::Value::Object(patch),
                    });
                }
                if let Some(slot) = self.mirrors.sources.get_mut(i) {
                    *slot = bus_now;
                }
            } else if let (Some(model_source), Some(params)) =
                (snap.sources.get(i), self.params.sources.get(i))
            {
                let model_now = Self::source_from_model(model_source, params);
                if Some(&model_now) != self.mirrors.sources_model.get(i) {
                    self.seed_source(i, &model_now);
                    if let Some(slot) = self.mirrors.sources.get_mut(i) {
                        *slot = model_now.clone();
                    }
                    if let Some(slot) = self.mirrors.sources_model.get_mut(i) {
                        *slot = model_now;
                    }
                }
                // Read-only mirrors (FilePlayer loaded/duration/error, file path).
                self.publish_source_mirrors(i, model_source);
            }
        }
    }

    fn seed_source(&self, i: usize, doc: &SourceState) {
        let Some(source) = self.params.sources.get(i) else {
            return;
        };
        let get = |key: &str| doc.0.get(key);
        if let Some(v) = get("visible").and_then(|v| v.as_bool()) {
            let _ = self.bus.set(source.visible, Value::Bool(v));
        }
        if let Some(v) = get("renderEnabled").and_then(|v| v.as_bool()) {
            let _ = self.bus.set(source.render_enabled, Value::Bool(v));
        }
        if let Some(v) = get("alpha").and_then(|v| v.as_f64()) {
            let _ = self.bus.set(source.alpha, Value::F32(v as f32));
        }
        if let Some(idx) = get("style").and_then(|v| v.as_str()).and_then(|s| {
            ["Direct", "HV_ThetaR", "Centered"]
                .iter()
                .position(|w| *w == s)
        }) {
            let _ = self.bus.set(source.style, Value::Enum(idx as u32));
        }
        for (leaf, id) in &source.extras {
            let Some(value) = get(source_json_key(leaf)) else {
                continue;
            };
            match value {
                serde_json::Value::Bool(b) => {
                    let _ = self.bus.set(*id, Value::Bool(*b));
                }
                serde_json::Value::Number(n) => {
                    // The declaration decides the kind; i32 params get ints.
                    if matches!(leaf.as_str(), "size" | "border") {
                        let _ = self
                            .bus
                            .set(*id, Value::I32(n.as_i64().unwrap_or(0) as i32));
                    } else {
                        let _ = self
                            .bus
                            .set(*id, Value::F32(n.as_f64().unwrap_or(0.0) as f32));
                    }
                }
                serde_json::Value::String(s) => {
                    if let Some(idx) = source_enum_index(leaf, s) {
                        let _ = self.bus.set(*id, Value::Enum(idx));
                    } else {
                        let _ = self.bus.set_text(*id, s);
                    }
                }
                serde_json::Value::Array(a) if a.len() == 2 => {
                    let x = a[0].as_f64().unwrap_or(0.0) as f32;
                    let y = a[1].as_f64().unwrap_or(0.0) as f32;
                    let _ = self.bus.set(*id, Value::Vec2([x, y]));
                }
                _ => {}
            }
        }
    }

    fn publish_source_mirrors(&mut self, i: usize, model: &serde_json::Value) {
        let Some(source) = self.params.sources.get(i) else {
            return;
        };
        let mirrors: Vec<(String, ParamId)> = source.mirrors.clone();
        for (leaf, id) in mirrors {
            match leaf.as_str() {
                "file" => {
                    let text = model
                        .get("file")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    self.set_text_cached(id, &text);
                }
                "loaded" => {
                    let loaded = model
                        .get("loaded")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let _ = self.bus.set(id, Value::Bool(loaded));
                }
                "duration_s" => {
                    let d = model.get("durationS").or_else(|| model.get("duration_s"));
                    let _ = self.bus.set(
                        id,
                        Value::F32(d.and_then(|v| v.as_f64()).unwrap_or(0.0) as f32),
                    );
                }
                "error" => {
                    let text = model
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    self.set_text_cached(id, &text);
                }
                "status" => {
                    let text = model
                        .get("status")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    self.set_text_cached(id, &text);
                }
                _ => {}
            }
        }
    }

    fn set_text_cached(&mut self, id: ParamId, text: &str) {
        if self.texts.get(&id).map(String::as_str) != Some(text) {
            let _ = self.bus.set_text(id, text);
            self.texts.insert(id, text.to_string());
        }
    }

    fn sync_verbose(&mut self) {
        let bus_now = bool_of(self.bus.get(self.params.report_verbose));
        if bus_now != self.mirrors.verbose {
            self.reporter.set_verbose(bus_now);
            self.mirrors.verbose = bus_now;
        }
    }

    // -------------------------------------------------------------------- publications

    fn publish_telemetry(&mut self, snap: &UiSnapshot, writers: &Writers<'_>) {
        let total: usize = self.shape.total_portals().max(1);

        // pose + link, all portals in slot order
        let mut pose = vec![f32::NAN; total * 4];
        let mut link = vec![f32::NAN; total * 4];
        let mut slot = 0usize;
        for column in &snap.columns {
            for portal in &column.portals {
                if slot >= total {
                    break;
                }
                let o = slot * 4;
                pose[o] = portal.position.x;
                pose[o + 1] = portal.position.y;
                if let Some(live) = portal.live_position {
                    pose[o + 2] = live.x;
                    pose[o + 3] = live.y;
                }
                link[o] = portal.last_rx_age_ms.map_or(f32::NAN, |v| v as f32);
                link[o + 1] = portal.last_tx_age_ms.map_or(f32::NAN, |v| v as f32);
                let (state, score) = self
                    .health
                    .get(&(column.index as u8, portal.target))
                    .copied()
                    .unwrap_or((0, 0));
                link[o + 2] = f32::from(state);
                link[o + 3] = f32::from(score);
                slot += 1;
            }
        }
        if let Some(writer) = &writers.pose {
            writer.push_f32_block(&pose);
        }
        if self.ticks % 3 == 0 {
            if let Some(writer) = &writers.link {
                writer.push_f32_block(&link);
            }
        }

        // columns
        let mut columns = vec![f32::NAN; self.shape.columns.len().max(1) * 4];
        for (i, column) in snap
            .columns
            .iter()
            .enumerate()
            .take(self.shape.columns.len())
        {
            let o = i * 4;
            columns[o] = column.stats.last_rx_age_ms.map_or(f32::NAN, |v| v as f32);
            columns[o + 1] = column.stats.last_tx_age_ms.map_or(f32::NAN, |v| v as f32);
            columns[o + 2] = column.stats.outbox_size as f32;
            columns[o + 3] = if column.stats.connected { 1.0 } else { 0.0 };
        }
        if let Some(writer) = &writers.columns {
            writer.push_f32_block(&columns);
        }

        // the selected portal, at bridge rate — the ring is the sparkline's history
        let (col, target) = self.selection();
        if let Some(portal) = snap
            .columns
            .get(col)
            .and_then(|c| c.portals.iter().find(|p| p.target == target))
        {
            let nan = f32::NAN;
            let opt =
                |v: Option<glam::Vec2>, i: usize| v.map_or(nan, |v| if i == 0 { v.x } else { v.y });
            let row = [
                portal.position.x,
                portal.position.y,
                portal.polar.x,
                portal.polar.y,
                portal.axes.x,
                portal.axes.y,
                opt(portal.live_axes, 0),
                opt(portal.live_axes, 1),
                opt(portal.live_position, 0),
                opt(portal.live_position, 1),
                opt(portal.live_target_position, 0),
                opt(portal.live_target_position, 1),
                portal.mc[0].reported_position.map_or(nan, |v| v as f32),
                portal.mc[0].reported_target.map_or(nan, |v| v as f32),
                portal.mc[1].reported_position.map_or(nan, |v| v as f32),
                portal.mc[1].reported_target.map_or(nan, |v| v as f32),
                if portal.in_target_position { 1.0 } else { 0.0 },
                portal.last_rx_age_ms.map_or(nan, |v| v as f32),
            ];
            if let Some(writer) = &writers.selected {
                writer.push_f32_block(&row);
            }
        }

        // OSC messages per tick
        if let Some(writer) = &writers.osc {
            writer.push_f32_block(&[snap.osc_messages_per_tick as f32]);
        }

        // preview, at ~15 Hz
        if self.ticks % 2 == 0
            && snap.preview.width == self.shape.resolution.0
            && snap.preview.height == self.shape.resolution.1
            && snap.preview.width > 0
        {
            let mut bytes = Vec::with_capacity(snap.preview.data.len());
            for v in &snap.preview.data {
                bytes.push((v.clamp(0.0, 1.0) * 255.0) as u8);
            }
            if let Some(writer) = &writers.preview {
                writer.push_raw(&bytes);
            }
        }
    }

    fn publish_observed(&mut self, snap: &UiSnapshot) {
        // Per-column counters (change-detected by the bus's own delta encoding; cheap).
        for (i, column) in snap.columns.iter().enumerate() {
            let Some(c) = self.params.columns.get(i) else {
                continue;
            };
            let _ = self
                .bus
                .set(c.connected, Value::Bool(column.stats.connected));
            let _ = self
                .bus
                .set(c.tx_count, Value::I64(column.stats.tx_count as i64));
            let _ = self
                .bus
                .set(c.rx_count, Value::I64(column.stats.rx_count as i64));
            let _ = self
                .bus
                .set(c.ack_timeouts, Value::I64(column.stats.ack_timeouts as i64));
            let _ = self.bus.set(
                c.decode_errors,
                Value::I64(column.stats.decode_errors as i64),
            );
            let device_description_id = c.device_description;
            let description = column.stats.device_description.clone();
            self.set_text_cached(device_description_id, &description);
        }

        let observed = ObservedState {
            osc_running: snap.osc_running,
            osc_port: i32::from(snap.osc_port),
            rest_running: snap.rest_running,
            rest_port: i32::from(snap.rest_port),
            ..self.mirrors.observed.clone()
        };
        if observed != self.mirrors.observed {
            let p = &self.params;
            let _ = self
                .bus
                .set(p.servers_osc_running, Value::Bool(observed.osc_running));
            let _ = self
                .bus
                .set(p.servers_osc_port, Value::I32(observed.osc_port));
            let _ = self
                .bus
                .set(p.servers_rest_running, Value::Bool(observed.rest_running));
            let _ = self
                .bus
                .set(p.servers_rest_port, Value::I32(observed.rest_port));
            self.mirrors.observed = observed;
        }

        // Tx/Rx rate EMA, published at ≤4 Hz.
        let totals = snap.columns.iter().fold((0u64, 0u64), |acc, c| {
            (acc.0 + c.stats.tx_count, acc.1 + c.stats.rx_count)
        });
        let dt = self.last_rates_at.elapsed().as_secs_f32();
        if dt >= 0.25 {
            let tx_rate = (totals.0.saturating_sub(self.prev_totals.0)) as f32 / dt;
            let rx_rate = (totals.1.saturating_sub(self.prev_totals.1)) as f32 / dt;
            let alpha = 0.3;
            self.tx_rx_ema.0 += alpha * (tx_rate - self.tx_rx_ema.0);
            self.tx_rx_ema.1 += alpha * (rx_rate - self.tx_rx_ema.1);
            self.prev_totals = totals;
            self.last_rates_at = Instant::now();
            let _ = self
                .bus
                .set(self.params.stats_tx_per_s, Value::F32(self.tx_rx_ema.0));
            let _ = self
                .bus
                .set(self.params.stats_rx_per_s, Value::F32(self.tx_rx_ema.1));
        }
    }

    fn slow_jobs(&mut self, _snap: &UiSnapshot) {
        let diag = self.reporter.snapshot();
        self.health = diag
            .portals
            .iter()
            .map(|p| ((p.col, p.portal), (portal_state_index(p.state), p.score)))
            .collect();
        let faulty = diag
            .portals
            .iter()
            .filter(|p| matches!(p.state, PortalState::Faulty | PortalState::Silent))
            .count() as i32;
        let _ = self
            .bus
            .set(self.params.health_faulty_units, Value::I32(faulty));
        let session_file_id = self.params.report_session_file;
        let session_file = diag.session_file.clone();
        self.set_text_cached(session_file_id, &session_file);
        let _ = self.bus.set(
            self.params.report_file_bytes,
            Value::I64(diag.file_bytes as i64),
        );
        let _ = self.bus.set(
            self.params.report_dropped,
            Value::I64(diag.dropped_events as i64),
        );
        // Reporter-side verbose is authoritative when the bus didn't just change it.
        if diag.verbose != self.mirrors.verbose {
            let _ = self
                .bus
                .set(self.params.report_verbose, Value::Bool(diag.verbose));
            self.mirrors.verbose = diag.verbose;
        }
        *self.shared.diag.lock().unwrap() = diag;
        *self.shared.ports.lock().unwrap() = router_core::rs485::list_serial_ports();
    }
}

fn source_enum_index(leaf: &str, s: &str) -> Option<u32> {
    let table: &[&str] = match leaf {
        "gradient_type" => &["Radial", "Horizontal", "Vertical"],
        "wave" => &["Triangle", "Sine", "Sawtooth"],
        "loop_mode" => &["Loop", "Ping Pong", "None"],
        _ => return None,
    };
    table.iter().position(|w| *w == s).map(|i| i as u32)
}
