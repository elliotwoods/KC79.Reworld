//! RouterRS GUI: iced front-end over the router-core runtime.
//!
//! Usage: router-app [--config <path>] [--simulate] [--report-dir <dir>]
//!                   [--verbose] [--sim-dead <ids>] [--sim-noisy <ids>]
//!                   [--sim-drop <0..1>] [--sim-corrupt <0..1>]

mod icons;
mod message;
mod selection;
mod theme;
mod views;
mod widgets;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::vec2;
use iced::keyboard;
use iced::{Element, Subscription, Task};
use router_core::config::AppConfig;
use router_core::runtime::{Command, McCommand, Query, RuntimeConfig, RuntimeHandle, Scope, UiSnapshot};
use router_core::sim::SimConfig;
use router_report::{
    DiagnosticsSnapshot, PortalState, ReportConfig, Reporter, ReporterHandle, SessionInfo,
};

use message::Message;
use selection::{PortalSub, Selection, TopModule};
use views::Rates;

fn main() -> iced::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let get_value = |flag: &str| -> Option<String> {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let has_flag = |flag: &str| args.iter().any(|a| a == flag);

    let config_path = PathBuf::from(get_value("--config").unwrap_or_else(|| "config.json".into()));
    let app_config = AppConfig::load(&config_path).unwrap_or_else(|_| AppConfig::from_json(serde_json::json!({})));

    let simulate = if has_flag("--simulate") {
        let parse_ids = |flag: &str| -> Vec<u8> {
            get_value(flag)
                .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect())
                .unwrap_or_default()
        };
        let mut sim = SimConfig::default();
        sim.dead_portals = parse_ids("--sim-dead");
        sim.noisy_portals = parse_ids("--sim-noisy");
        if let Some(rate) = get_value("--sim-drop").and_then(|v| v.parse().ok()) {
            sim.drop_rate = rate;
        }
        if let Some(rate) = get_value("--sim-corrupt").and_then(|v| v.parse().ok()) {
            sim.corrupt_rate = rate;
        }
        Some(sim)
    } else {
        None
    };

    let report_config = ReportConfig {
        dir: PathBuf::from(get_value("--report-dir").unwrap_or_else(|| "reports".into())),
        verbose: has_flag("--verbose"),
        ..Default::default()
    };
    let session = SessionInfo {
        app_version: format!("router-app {}", env!("CARGO_PKG_VERSION")),
        host: std::env::var("COMPUTERNAME").unwrap_or_else(|_| "unknown".into()),
        config: serde_json::json!({
            "columns": app_config.installation.arrangement.columns,
            "rows": app_config.installation.arrangement.rows,
            "simulate": simulate.is_some(),
        }),
    };
    let (reporter, reporter_handle) = Reporter::start(report_config, session)
        .expect("failed to start the session reporter");

    let runtime = router_core::runtime::spawn(RuntimeConfig {
        app_config,
        config_path: Some(config_path),
        simulate,
        reporter: reporter.clone(),
    });

    let app = App::new(runtime, reporter, reporter_handle);

    iced::application("Router RS", App::update, App::view)
        .subscription(App::subscription)
        .theme(|_| theme::theme())
        .font(icons::FONT_BYTES)
        .antialiasing(true)
        .window_size((1600.0, 960.0))
        .run_with(move || (app, Task::none()))
}

struct App {
    runtime: RuntimeHandle,
    reporter: Reporter,
    _reporter_handle: ReporterHandle,
    snap: Arc<UiSnapshot>,
    diag: Arc<DiagnosticsSnapshot>,
    selection: Selection,
    center: TopModule,
    edits: HashMap<&'static str, String>,
    serial_ports: Vec<String>,
    marker_text: String,
    ticks: u64,
    // aggregate Tx/Rx rate tracking (EMA over snapshot counter deltas)
    rates: Rates,
    prev_totals: (u64, u64),
    last_tick: Instant,
    // (col, portal) -> health state / score, rebuilt when diag refreshes
    health: HashMap<(u8, u8), PortalState>,
    scores: HashMap<(u8, u8), u8>,
    // per-column outbox display value, held after the last non-empty sighting
    // so the indicator doesn't strobe at the transmit cadence
    outbox_hold: HashMap<usize, (usize, Instant)>,
    outbox_display: Vec<usize>,
}

/// How long the outbox indicator stays lit after the outbox last held packets.
const OUTBOX_HOLD: Duration = Duration::from_millis(1500);

impl App {
    fn new(runtime: RuntimeHandle, reporter: Reporter, reporter_handle: ReporterHandle) -> Self {
        Self {
            snap: runtime.snapshot(),
            diag: reporter.snapshot(),
            runtime,
            reporter,
            _reporter_handle: reporter_handle,
            selection: Selection::Module(TopModule::Installation),
            center: TopModule::Installation,
            edits: HashMap::new(),
            serial_ports: router_core::rs485::list_serial_ports(),
            marker_text: String::new(),
            ticks: 0,
            rates: Rates::default(),
            prev_totals: (0, 0),
            last_tick: Instant::now(),
            health: HashMap::new(),
            scores: HashMap::new(),
            outbox_hold: HashMap::new(),
            outbox_display: Vec::new(),
        }
    }

    fn update_outbox_display(&mut self) {
        let now = Instant::now();
        self.outbox_display.clear();
        for column in &self.snap.columns {
            let size = column.stats.outbox_size;
            let display = if size > 0 {
                self.outbox_hold.insert(column.index, (size, now));
                size
            } else {
                match self.outbox_hold.get(&column.index) {
                    Some((held, last)) if now.duration_since(*last) < OUTBOX_HOLD => *held,
                    _ => {
                        self.outbox_hold.remove(&column.index);
                        0
                    }
                }
            };
            self.outbox_display.push(display);
        }
    }

    fn update_rates(&mut self) {
        let tx: u64 = self.snap.columns.iter().map(|c| c.stats.tx_count).sum();
        let rx: u64 = self.snap.columns.iter().map(|c| c.stats.rx_count).sum();
        let dt = self.last_tick.elapsed().as_secs_f32();
        self.last_tick = Instant::now();
        if dt <= 0.0 {
            return;
        }
        let (prev_tx, prev_rx) = self.prev_totals;
        self.prev_totals = (tx, rx);
        // counters can reset (Clear counters); treat a decrease as zero delta
        let tx_rate = tx.saturating_sub(prev_tx) as f32 / dt;
        let rx_rate = rx.saturating_sub(prev_rx) as f32 / dt;
        const EMA: f32 = 0.05;
        self.rates.tx_per_s += (tx_rate - self.rates.tx_per_s) * EMA;
        self.rates.rx_per_s += (rx_rate - self.rates.rx_per_s) * EMA;
    }

    fn rebuild_health_maps(&mut self) {
        self.health.clear();
        self.scores.clear();
        for portal in &self.diag.portals {
            self.health.insert((portal.col, portal.portal), portal.state);
            self.scores.insert((portal.col, portal.portal), portal.score);
        }
    }

    fn send(&self, command: Command) {
        self.runtime.send(command);
    }

    fn selected_portal(&self) -> Option<(usize, u8)> {
        self.selection.portal()
    }

    fn selected_axis(&self) -> usize {
        match self.selection {
            Selection::PortalSub { sub: PortalSub::Axis(axis), .. } => axis,
            _ => 0,
        }
    }

    fn update(&mut self, message: Message) {
        match message {
            Message::Tick => {
                self.ticks += 1;
                self.snap = self.runtime.snapshot();
                self.update_rates();
                self.update_outbox_display();
                if self.ticks % 60 == 0 {
                    self.diag = self.reporter.snapshot();
                    self.rebuild_health_maps();
                }
            }
            Message::Select(selection) => {
                self.selection = selection;
                self.edits.clear();
            }
            Message::SelectCenter(module) => {
                self.center = module;
                self.selection = Selection::Module(module);
                self.edits.clear();
            }

            Message::Action(scope, action) => self.send(Command::PerformAction { scope, action }),
            Message::Poll(scope) => self.send(Command::Poll(scope)),
            Message::HomeAndZero => self.send(Command::HomeAndZeroLocal),
            Message::RebuildColumns => self.send(Command::RebuildColumns),

            Message::PilotDragTo { col, target, position } => {
                self.send(Command::SetPilotPosition { col, portal: target, position });
            }
            Message::PilotSetAxis { col, target, axis, value } => {
                // keep the other axis where it is
                if let Some(portal) = self
                    .snap
                    .columns
                    .get(col)
                    .and_then(|c| c.portals.iter().find(|p| p.target == target))
                {
                    let axes = if axis == 0 {
                        vec2(value, portal.axes.y)
                    } else {
                        vec2(portal.axes.x, value)
                    };
                    self.send(Command::SetPilotAxes { col, portal: target, axes });
                }
            }
            Message::PilotOffset { col, target, offset } => {
                self.send(Command::SetPilotOffset { col, portal: target, offset });
            }
            Message::PilotPush => {
                if let Some((col, portal)) = self.selected_portal() {
                    self.send(Command::Push { col, portal });
                }
            }
            Message::PilotPollPosition => {
                if let Some((col, portal)) = self.selected_portal() {
                    self.send(Command::PollPosition { col, portal });
                }
            }
            Message::PilotResetLocal => {
                if let Some((col, portal)) = self.selected_portal() {
                    self.send(Command::ResetLocal { col, portal });
                }
            }
            Message::PilotUnwind => {
                if let Some((col, portal)) = self.selected_portal() {
                    self.send(Command::Unwind(Scope::Portal(col, portal)));
                }
            }
            Message::PilotTakeCurrent => {
                if let Some((col, portal)) = self.selected_portal() {
                    self.send(Command::TakeCurrentPosition { col, portal });
                }
            }
            Message::PilotSeeThrough => {
                if let Some((col, portal)) = self.selected_portal() {
                    self.send(Command::SeeThroughLocal { col, portal });
                }
            }

            Message::Mc { axis, kind } => {
                if let Some((col, portal)) = self.selected_portal() {
                    self.send(Command::Mc { col, portal, axis, kind });
                }
            }
            Message::MdTestRoutine { axis } => {
                if let Some((col, portal)) = self.selected_portal() {
                    self.send(Command::MdTestRoutine { col, portal, axis });
                }
            }
            Message::MdTestTimer { axis } => {
                if let Some((col, portal)) = self.selected_portal() {
                    self.send(Command::MdTestTimer { col, portal, axis });
                }
            }

            Message::ClearOutbox(col) => self.send(Command::ClearOutbox(col)),
            Message::ClearCounters(col) => self.send(Command::Rs485ClearCounters { col }),
            Message::Disconnect(col) => self.send(Command::Rs485Disconnect { col }),
            Message::ConnectSerial(col, port) => {
                self.send(Command::Rs485Connect {
                    col,
                    settings: serde_json::json!({ "deviceType": "Serial", "address": port }),
                });
            }
            Message::ConnectTcp(col, host) => {
                self.send(Command::Rs485Connect {
                    col,
                    settings: serde_json::json!({ "deviceType": "TCP", "address": host }),
                });
            }
            Message::RefreshPorts => {
                self.serial_ports = router_core::rs485::list_serial_ports();
            }

            Message::ToggleImageEnabled(enabled) => self.send(Command::SetImageEnabled(enabled)),
            Message::TransmitModeSelected(mode) => {
                use router_core::config::ImageTransmit;
                let mode = match mode.as_str() {
                    "Keyframe" => ImageTransmit::Keyframe,
                    "Disabled" => ImageTransmit::Disabled,
                    _ => ImageTransmit::Individual,
                };
                self.send(Command::SetTransmitMode(mode));
            }
            Message::ToggleFlipped(flipped) => {
                let (columns, rows, width, _) = self.snap.arrangement;
                self.send(Command::SetArrangement { columns, rows, column_width: width, flipped });
            }
            Message::ToggleVelocities(velocities) => {
                self.send(Command::SetMessaging {
                    period_s: self.snap.period_s,
                    keyframe_batch_size: self.snap.keyframe_batch_size,
                    keyframe_velocities: velocities,
                });
            }
            Message::ToggleScheduledPoll(col, enabled) => {
                let period_s = self
                    .snap
                    .columns
                    .get(col)
                    .map(|c| c.scheduled_poll_period_s)
                    .unwrap_or(60.0);
                self.send(Command::SetScheduledPoll { col, enabled, period_s });
            }
            Message::TogglePollRegularly(enabled) => {
                if let Some((col, portal)) = self.selected_portal() {
                    let interval_s = self
                        .snap
                        .columns
                        .get(col)
                        .and_then(|c| c.portals.iter().find(|p| p.target == portal))
                        .map(|p| p.poll_interval_s)
                        .unwrap_or(1.0);
                    self.send(Command::SetPollRegularly { col, portal, enabled, interval_s });
                }
            }
            Message::ToggleSendPeriodically(enabled) => {
                if let Some((col, portal)) = self.selected_portal() {
                    self.send(Command::SetPilotSendPeriodically { col, portal, enabled });
                }
            }

            Message::SourceAdd(type_name) => self.send(Command::SourceAdd { type_name }),
            Message::SourceRemove(index) => self.send(Command::SourceRemove { index }),
            Message::SourceParam(index, key, value) => {
                self.send(Command::SourceSetParams {
                    index,
                    params: serde_json::json!({ key: value }),
                });
            }
            Message::SourceFileDialog(index) => {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Video", &["mp4", "mov", "avi", "mkv", "webm"])
                    .pick_file()
                {
                    self.send(Command::SourceSetParams {
                        index,
                        params: serde_json::json!({ "file": path.display().to_string() }),
                    });
                }
            }
            Message::FwUploadDialog(col) => {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Firmware binary", &["bin"])
                    .pick_file()
                {
                    self.send(Command::FwUpload { col, path });
                }
            }
            Message::FwErase(col) => self.send(Command::FwErase { col }),
            Message::FwRun(col) => self.send(Command::FwRun { col }),

            Message::Edit(id, value) => {
                self.edits.insert(id, value);
            }
            Message::Submit(id) => self.apply_edit(id),

            Message::WriteSummaryNow => {
                self.reporter.write_summary_now();
            }
            Message::ToggleVerbose => {
                self.reporter.set_verbose(!self.reporter.is_verbose());
            }
            Message::MarkerText(value) => self.marker_text = value,
            Message::AddMarker => {
                if !self.marker_text.is_empty() {
                    self.send(Command::Marker(self.marker_text.clone()));
                    self.marker_text.clear();
                }
            }
            Message::SaveConfig => self.send(Command::SaveConfig),
        }
    }

    fn apply_edit(&mut self, id: &'static str) {
        let Some(raw) = self.edits.remove(id) else { return };
        let raw = raw.trim().to_string();

        let parse_usize = |s: &str| s.parse::<usize>().ok();
        let parse_f32 = |s: &str| s.parse::<f32>().ok();
        let parse_i32 = |s: &str| s.parse::<i32>().ok();

        match id {
            "arr.columns" | "arr.rows" | "arr.width" => {
                let (mut columns, mut rows, mut width, flipped) = self.snap.arrangement;
                let Some(value) = parse_usize(&raw) else { return };
                match id {
                    "arr.columns" => columns = value,
                    "arr.rows" => rows = value,
                    _ => width = value,
                }
                self.send(Command::SetArrangement { columns, rows, column_width: width, flipped });
            }
            "msg.period" => {
                if let Some(period_s) = parse_f32(&raw) {
                    self.send(Command::SetMessaging {
                        period_s,
                        keyframe_batch_size: self.snap.keyframe_batch_size,
                        keyframe_velocities: self.snap.keyframe_velocities,
                    });
                }
            }
            "msg.batch" => {
                if let Some(batch) = parse_usize(&raw) {
                    self.send(Command::SetMessaging {
                        period_s: self.snap.period_s,
                        keyframe_batch_size: batch,
                        keyframe_velocities: self.snap.keyframe_velocities,
                    });
                }
            }
            "col.poll.period" => {
                if let (Selection::Column(col), Some(period_s)) = (self.selection, parse_f32(&raw)) {
                    let enabled = self
                        .snap
                        .columns
                        .get(col)
                        .map(|c| c.scheduled_poll_enabled)
                        .unwrap_or(false);
                    self.send(Command::SetScheduledPoll { col, enabled, period_s });
                }
            }
            "poll.interval" => {
                if let (Some((col, portal)), Some(interval_s)) = (self.selected_portal(), parse_f32(&raw)) {
                    let enabled = self
                        .snap
                        .columns
                        .get(col)
                        .and_then(|c| c.portals.iter().find(|p| p.target == portal))
                        .map(|p| p.poll_regularly)
                        .unwrap_or(false);
                    self.send(Command::SetPollRegularly { col, portal, enabled, interval_s });
                }
            }
            "mc.maxv" | "mc.acc" | "mc.minv" => {
                if let (Some((col, portal)), Some(value)) = (self.selected_portal(), parse_i32(&raw)) {
                    let axis = self.selected_axis();
                    if let Some(mc) = self
                        .snap
                        .columns
                        .get(col)
                        .and_then(|c| c.portals.iter().find(|p| p.target == portal))
                        .map(|p| &p.mc[axis.min(1)])
                    {
                        let (mut max_velocity, mut acceleration, mut min_velocity) =
                            (mc.max_velocity, mc.acceleration, mc.min_velocity);
                        match id {
                            "mc.maxv" => max_velocity = value,
                            "mc.acc" => acceleration = value,
                            _ => min_velocity = value,
                        }
                        self.send(Command::SetMotionProfile {
                            col,
                            portal,
                            axis,
                            max_velocity,
                            acceleration,
                            min_velocity,
                        });
                    }
                }
            }
            "mds.current" => {
                if let (Some((col, portal)), Some(amps)) = (self.selected_portal(), parse_f32(&raw)) {
                    self.send(Command::SetPortalCurrent { col, portal, amps });
                }
            }
            "mds.microstep" => {
                if let (Some((col, portal)), Some(resolution)) =
                    (self.selected_portal(), raw.parse::<u32>().ok())
                {
                    self.send(Command::SetPortalMicrostep { col, portal, resolution });
                }
            }
            "tcp.host" => {
                if let Selection::Column(col) = self.selection {
                    if !raw.is_empty() {
                        // accept host or host:port
                        let (host, port) = match raw.split_once(':') {
                            Some((h, p)) => (h.to_string(), p.parse::<u16>().ok()),
                            None => (raw.clone(), None),
                        };
                        let mut settings = serde_json::json!({ "deviceType": "TCP", "address": host });
                        if let Some(port) = port {
                            settings["port"] = serde_json::json!(port);
                        }
                        self.send(Command::Rs485Connect { col, settings });
                    }
                }
            }
            _ => {}
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let ctx = views::Ctx {
            snap: &self.snap,
            diag: &self.diag,
            selection: self.selection,
            center: self.center,
            edits: &self.edits,
            serial_ports: &self.serial_ports,
            marker_text: &self.marker_text,
            rates: self.rates,
            health: &self.health,
            scores: &self.scores,
            outbox_display: &self.outbox_display,
        };
        views::root(ctx)
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            iced::time::every(Duration::from_millis(16)).map(|_| Message::Tick),
            keyboard::on_key_press(|key, modifiers| {
                if modifiers.command() || modifiers.alt() {
                    return None;
                }
                match key.as_ref() {
                    keyboard::Key::Character("r") => Some(Message::PilotResetLocal),
                    keyboard::Key::Character("u") => Some(Message::PilotUnwind),
                    keyboard::Key::Character("m") => Some(Message::PilotPush),
                    keyboard::Key::Named(keyboard::key::Named::Space) => None, // handled below
                    _ => None,
                }
            }),
        ])
    }
}

// Silence unused warning: Query is part of the public runtime API the GUI
// may use for future panels.
#[allow(dead_code)]
fn _query_type_check(q: Query) -> Query {
    q
}

#[allow(dead_code)]
fn _mc_check(kind: McCommand) -> McCommand {
    kind
}
