//! The runtime: a single model thread owning the Installation + Renderer,
//! consuming Commands from all producers (GUI / OSC / REST), publishing
//! UiSnapshots. No shared mutable model state — an actor, like the C++
//! app's single-threaded update loop.

pub mod command;
pub mod snapshot;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glam::vec2;
use router_proto::repeater::{RepeaterTarget, RepeaterVerb};
use router_report::Reporter;

use crate::config::AppConfig;
use crate::image::{RenderSettings, Renderer};
use crate::model::installation::Installation;
use crate::model::pilot::LeadingControl;
use crate::sim::{SimBus, SimConfig};

pub use command::{Command, McCommand, Query, Scope};
pub use snapshot::{ColumnSnapshot, PortalSnapshot, UiSnapshot};

const TICK: Duration = Duration::from_millis(16);

pub struct RuntimeConfig {
    pub app_config: AppConfig,
    pub config_path: Option<PathBuf>,
    /// Replace every column's device with an in-process simulated bus.
    pub simulate: Option<SimConfig>,
    pub reporter: Reporter,
}

pub struct RuntimeHandle {
    commands: Sender<Command>,
    snapshot: Arc<Mutex<Arc<UiSnapshot>>>,
    pub reporter: Reporter,
    join: Option<std::thread::JoinHandle<()>>,
    server_threads: Vec<std::thread::JoinHandle<()>>,
    shutdown_flag: Arc<AtomicBool>,
}

impl RuntimeHandle {
    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    pub fn command_sender(&self) -> Sender<Command> {
        self.commands.clone()
    }

    pub fn snapshot(&self) -> Arc<UiSnapshot> {
        self.snapshot.lock().unwrap().clone()
    }

    /// The shared snapshot slot, for a consumer thread that outlives this handle's borrow
    /// (e.g. a GUI bridge). Reading it is one mutex-guarded `Arc` clone, same as `snapshot`.
    pub fn snapshot_slot(&self) -> Arc<Mutex<Arc<UiSnapshot>>> {
        self.snapshot.clone()
    }

    pub fn shutdown(mut self) {
        self.shutdown_impl();
    }

    fn shutdown_impl(&mut self) {
        self.shutdown_flag.store(true, Ordering::Release);
        let _ = self.commands.send(Command::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        // server threads exit on their own (they poll the shutdown flag)
        for join in self.server_threads.drain(..) {
            let _ = join.join();
        }
    }
}

impl Drop for RuntimeHandle {
    fn drop(&mut self) {
        if self.join.is_some() {
            self.shutdown_impl();
        }
    }
}

pub fn spawn(config: RuntimeConfig) -> RuntimeHandle {
    let (command_tx, command_rx) = channel::<Command>();
    let snapshot: Arc<Mutex<Arc<UiSnapshot>>> =
        Arc::new(Mutex::new(Arc::new(UiSnapshot::default())));
    let shutdown_flag = Arc::new(AtomicBool::new(false));
    let osc_message_count = Arc::new(AtomicUsize::new(0));

    // servers
    let mut server_threads = Vec::new();
    let rest_running = config.app_config.rest.enabled;
    let rest_port = config.app_config.rest.port;
    if rest_running {
        server_threads.push(crate::servers::rest::spawn(
            rest_port,
            command_tx.clone(),
            shutdown_flag.clone(),
        ));
    }
    let osc_running = config.app_config.osc.enabled;
    let osc_port = config.app_config.osc.port;
    if osc_running {
        server_threads.push(crate::servers::osc::spawn(
            osc_port,
            command_tx.clone(),
            shutdown_flag.clone(),
            osc_message_count.clone(),
        ));
    }

    let reporter = config.reporter.clone();
    let model_snapshot = snapshot.clone();
    let join = std::thread::Builder::new()
        .name("model".into())
        .spawn(move || {
            model_thread(
                config,
                command_rx,
                model_snapshot,
                ServerInfo {
                    rest_running,
                    rest_port,
                    osc_running,
                    osc_port,
                    osc_message_count,
                },
            )
        })
        .expect("spawn model thread");

    RuntimeHandle {
        commands: command_tx,
        snapshot,
        reporter,
        join: Some(join),
        server_threads,
        shutdown_flag,
    }
}

struct ServerInfo {
    rest_running: bool,
    rest_port: u16,
    osc_running: bool,
    osc_port: u16,
    osc_message_count: Arc<AtomicUsize>,
}

fn model_thread(
    config: RuntimeConfig,
    commands: Receiver<Command>,
    snapshot_slot: Arc<Mutex<Arc<UiSnapshot>>>,
    servers: ServerInfo,
) {
    let reporter = config.reporter;
    let mut installation =
        Installation::from_config(&config.app_config.installation, reporter.clone());
    let mut renderer = Renderer::from_config(&config.app_config.renderer_sources);
    let mut app_config = config.app_config;

    // simulated buses replace configured devices
    if let Some(sim_template) = &config.simulate {
        for column in &mut installation.columns {
            let mut sim_config = sim_template.clone();
            sim_config.portal_count = (column.count_x * column.count_y) as u8;
            column.rs485.open_device(Box::new(SimBus::new(sim_config)));
        }
    }

    let started = Instant::now();
    let mut generation = 0u64;
    let mut running = true;

    while running {
        let tick_start = Instant::now();

        // ---- commands ----
        while let Ok(command) = commands.try_recv() {
            if !handle_command(
                command,
                &mut installation,
                &mut renderer,
                &mut app_config,
                &reporter,
                config.config_path.as_deref(),
            ) {
                running = false;
            }
        }

        // ---- render + hardware update ----
        let (width, height) = installation.resolution();
        if width > 0 && height > 0 {
            renderer.render(&RenderSettings {
                width,
                height,
                time: started.elapsed().as_secs_f32(),
            });
        }
        installation.update(Some(&renderer.pixels));

        // ---- snapshot ----
        generation += 1;
        let ui = build_snapshot(
            &mut installation,
            &renderer,
            generation,
            &servers,
            &app_config,
        );
        *snapshot_slot.lock().unwrap() = Arc::new(ui);

        // ---- pace ----
        let elapsed = tick_start.elapsed();
        if elapsed < TICK {
            std::thread::sleep(TICK - elapsed);
        }
    }
}

/// Returns false when the runtime should stop.
fn handle_command(
    command: Command,
    installation: &mut Installation,
    renderer: &mut Renderer,
    app_config: &mut AppConfig,
    reporter: &Reporter,
    config_path: Option<&std::path::Path>,
) -> bool {
    use Command::*;
    match command {
        SetPilotPosition {
            col,
            portal,
            position,
        } => {
            if let Some(p) = installation.portal(col, portal) {
                p.pilot.set_position(position);
            }
        }
        SetPilotPolar { col, portal, polar } => {
            if let Some(p) = installation.portal(col, portal) {
                p.pilot.set_polar(polar);
            }
        }
        SetPilotAxes { col, portal, axes } => {
            if let Some(p) = installation.portal(col, portal) {
                p.pilot.set_axes(axes);
            }
        }
        SetAxesCyclicByIndex {
            col,
            portal_index,
            axes,
        } => {
            if let Some(column) = installation.column(col) {
                if let Some(p) = column.portals.get_mut(portal_index) {
                    p.pilot.set_axes_cyclic(axes);
                }
            }
        }
        Unwind(scope) => for_scope(installation, scope, |p| p.pilot.unwind()),
        ResetLocal { col, portal } => {
            if let Some(p) = installation.portal(col, portal) {
                p.pilot.reset_local();
            }
        }
        TakeCurrentPosition { col, portal } => {
            if let Some(p) = installation.portal(col, portal) {
                p.pilot.take_current_position();
            }
        }
        PilotAll { col, position } => {
            // Clamp to the unit circle (the C++ pad clamps at input; re-clamp for API callers).
            let position = if position.length() > 1.0 {
                position.normalize()
            } else {
                position
            };
            let first_pilot = |installation: &Installation, col: usize| {
                installation
                    .columns
                    .get(col)
                    .and_then(|c| c.portals.first())
                    .map(|p| {
                        let polar = crate::model::kinematics::position_to_polar(position);
                        let axes = p.pilot.polar_to_axes(polar);
                        (
                            p.pilot.axis_to_steps(axes.x, 0),
                            p.pilot.axis_to_steps(axes.y, 1),
                        )
                    })
            };
            match col {
                None => {
                    if let Some((a, b)) = first_pilot(installation, 0) {
                        installation.broadcast(&router_proto::commands::move_steps(a, b), true);
                    }
                }
                Some(col_index) => {
                    if let Some((a, b)) = first_pilot(installation, col_index) {
                        if let Some(column) = installation.column(col_index) {
                            column.broadcast(&router_proto::commands::move_steps(a, b), true);
                        }
                    }
                }
            }
        }
        PerformAction { scope, action } => match scope {
            Scope::All => installation.broadcast_action(action),
            Scope::Column(col) => {
                if let Some(column) = installation.column(col) {
                    column.broadcast_action(action);
                }
            }
            Scope::Portal(col, portal) => {
                if let Some(column) = installation.column(col) {
                    column.send_to_portal(portal, &action.body(), action.osc_address());
                    if let Some(p) = column.portal_by_target(portal) {
                        p.apply_action_effect(action);
                    }
                }
            }
        },
        Poll(scope) => match scope {
            Scope::All => installation.poll_all(),
            Scope::Column(col) => {
                if let Some(column) = installation.column(col) {
                    column.poll_all();
                }
            }
            Scope::Portal(col, portal) => {
                if let Some(column) = installation.column(col) {
                    column.send_to_portal(portal, &router_proto::commands::poll(), "poll");
                }
            }
        },
        PollPosition { col, portal } => {
            if let Some(column) = installation.column(col) {
                column.send_to_portal(portal, &router_proto::commands::poll_position(), "p");
            }
        }
        Push { col, portal } => {
            if let Some(column) = installation.column(col) {
                if let Some(p) = column.portal_by_target(portal) {
                    let body = p.pilot.move_message();
                    p.pilot.notify_values_sent();
                    column.send_to_portal(portal, &body, "m");
                }
            }
        }
        HomeAndZeroLocal => installation.home_hardware_and_zero_positions(),
        PushMotionProfileAll {
            max_velocity,
            acceleration,
        } => {
            for column in &mut installation.columns {
                for portal in &mut column.portals {
                    for mc in &mut portal.motion_control {
                        mc.max_velocity = max_velocity;
                        if let Some(acc) = acceleration {
                            mc.acceleration = acc;
                        }
                        let body = match acceleration {
                            Some(acc) => router_proto::Value::Map(vec![(
                                router_proto::value::key(mc.axis.motion_control_key()),
                                router_proto::Value::Map(vec![(
                                    router_proto::value::key("motionProfile"),
                                    router_proto::Value::Array(vec![
                                        router_proto::Value::from(max_velocity),
                                        router_proto::Value::from(acc),
                                    ]),
                                )]),
                            )]),
                            None => router_proto::Value::Map(vec![(
                                router_proto::value::key(mc.axis.motion_control_key()),
                                router_proto::Value::Map(vec![(
                                    router_proto::value::key("motionProfile"),
                                    router_proto::Value::Array(vec![router_proto::Value::from(
                                        max_velocity,
                                    )]),
                                )]),
                            )]),
                        };
                        let address = router_proto::commands::mc_address(mc.axis, "motionProfile");
                        let target = portal.target;
                        column.rs485.transmit(crate::rs485::Packet::from_body(
                            target as i8,
                            &body,
                            address,
                        ));
                    }
                }
            }
        }
        SetCurrentAll(amps) => {
            let body = router_proto::commands::mds_set_current(amps);
            for column in &mut installation.columns {
                for portal in &mut column.portals {
                    portal.motor_driver_settings.current_amps = amps;
                    portal.motor_driver_settings.mark_current_sent();
                    let target = portal.target;
                    column.rs485.transmit(crate::rs485::Packet::from_body(
                        target as i8,
                        &body,
                        "motorDriverSettings/setCurrent",
                    ));
                }
            }
        }
        Broadcast { body, collateable } => installation.broadcast(&body, collateable),
        SetTransmitMode(mode) => installation.transmit = mode,
        SetImageEnabled(enabled) => installation.image_enabled = enabled,
        RebuildColumns => installation.rebuild_columns(),
        ClearOutbox(col) => {
            if let Some(column) = installation.column(col) {
                column.rs485.clear_outbox();
            }
        }
        SetPilotOffset {
            col,
            portal,
            offset,
        } => {
            if let Some(p) = installation.portal(col, portal) {
                p.pilot.offset = offset;
            }
        }
        SetPilotSendPeriodically {
            col,
            portal,
            enabled,
        } => {
            if let Some(p) = installation.portal(col, portal) {
                p.pilot.send_periodically = enabled;
            }
        }
        SeeThroughLocal { col, portal } => {
            if let Some(p) = installation.portal(col, portal) {
                p.pilot.see_through();
            }
        }
        SetPollRegularly {
            col,
            portal,
            enabled,
            interval_s,
        } => {
            if let Some(p) = installation.portal(col, portal) {
                p.poll_regularly = enabled;
                p.poll_interval_s = interval_s;
            }
        }
        Mc {
            col,
            portal,
            axis,
            kind,
        } => {
            if let Some(column) = installation.column(col) {
                if let Some(p) = column.portal_by_target(portal) {
                    let mc = &p.motion_control[axis.min(1)];
                    let ax = mc.axis;
                    let (body, cmd) = match kind {
                        command::McCommand::ZeroCurrentPosition => (
                            router_proto::commands::mc_zero_current_position(ax),
                            "zeroCurrentPosition",
                        ),
                        command::McCommand::MeasureBacklash => (
                            router_proto::commands::mc_measure_backlash(ax, mc.measure),
                            "measureBacklash",
                        ),
                        command::McCommand::HomeRoutine => {
                            (router_proto::commands::mc_home(ax, mc.measure), "home")
                        }
                        command::McCommand::InitTimer => {
                            (router_proto::commands::mc_init_timer(ax), "initTimer")
                        }
                        command::McCommand::DeinitTimer => {
                            (router_proto::commands::mc_deinit_timer(ax), "deinitTimer")
                        }
                        command::McCommand::TestTimer => {
                            (router_proto::commands::mc_test_timer(ax), "testTimer")
                        }
                        command::McCommand::PushMotionProfile => (
                            router_proto::commands::mc_motion_profile(
                                ax,
                                mc.max_velocity,
                                mc.acceleration,
                                mc.min_velocity,
                            ),
                            "motionProfile",
                        ),
                    };
                    let address = router_proto::commands::mc_address(ax, cmd);
                    column.send_to_portal(portal, &body, &address);
                }
            }
        }
        SetMotionProfile {
            col,
            portal,
            axis,
            max_velocity,
            acceleration,
            min_velocity,
        } => {
            if let Some(p) = installation.portal(col, portal) {
                let mc = &mut p.motion_control[axis.min(1)];
                mc.max_velocity = max_velocity;
                mc.acceleration = acceleration;
                mc.min_velocity = min_velocity;
                // auto-push fires on the next portal update
            }
        }
        MdTestRoutine { col, portal, axis } => {
            if let Some(column) = installation.column(col) {
                let ax = router_proto::commands::Axis::from_index(axis.min(1));
                let body = router_proto::commands::md_test_routine(ax);
                column.send_to_portal(portal, &body, "motorDriver/testRoutine");
            }
        }
        MdTestTimer { col, portal, axis } => {
            if let Some(column) = installation.column(col) {
                if let Some(p) = column.portal_by_target(portal) {
                    let body = p.motor_driver[axis.min(1)].test_timer_message();
                    column.send_to_portal(portal, &body, "motorDriver/testTimer");
                }
            }
        }
        SetPortalCurrent { col, portal, amps } => {
            if let Some(column) = installation.column(col) {
                if let Some(p) = column.portal_by_target(portal) {
                    p.motor_driver_settings.current_amps = amps;
                    p.motor_driver_settings.mark_current_sent();
                    let body = router_proto::commands::mds_set_current(amps);
                    column.send_to_portal(portal, &body, "motorDriverSettings/setCurrent");
                }
            }
        }
        SetPortalMicrostep {
            col,
            portal,
            resolution,
        } => {
            if let Some(column) = installation.column(col) {
                if let Some(p) = column.portal_by_target(portal) {
                    p.motor_driver_settings.microstep_resolution = resolution;
                    p.motor_driver_settings.mark_microstep_sent();
                    let body = router_proto::commands::mds_set_microstep_resolution(resolution);
                    column.send_to_portal(
                        portal,
                        &body,
                        "motorDriverSettings/setMicrostepResolution",
                    );
                }
            }
        }
        SetScheduledPoll {
            col,
            enabled,
            period_s,
        } => {
            if let Some(column) = installation.column(col) {
                column.scheduled_poll_enabled = enabled;
                column.scheduled_poll_period_s = period_s;
            }
        }
        Rs485Connect { col, settings } => {
            if let Some(column) = installation.column(col) {
                column.rs485.open_from_settings(settings);
            }
        }
        Rs485Disconnect { col } => {
            if let Some(column) = installation.column(col) {
                column.rs485.close();
            }
        }
        Rs485ClearCounters { col } => {
            if let Some(column) = installation.column(col) {
                column.rs485.clear_counters();
            }
        }
        SetArrangement {
            columns,
            rows,
            column_width,
            flipped,
        } => {
            installation.columns_count = columns;
            installation.rows = rows;
            installation.column_width = column_width;
            installation.flipped = flipped;
            // like the C++, takes effect on "Rebuild columns"
        }
        SetMessaging {
            period_s,
            keyframe_batch_size,
            keyframe_velocities,
        } => {
            installation.period_s = period_s;
            installation.keyframe_batch_size = keyframe_batch_size;
            installation.keyframe_velocities = keyframe_velocities;
        }
        FwUpload { col, path } => match std::fs::read(&path) {
            Ok(firmware) => {
                // The GUI drives the blind broadcast path, whose `"ER"` erases the legacy
                // bank -- a bootloader old enough to be driven this way has its
                // application at 0x08006000 and nowhere else. An image linked for the v6
                // bank would program cleanly into the wrong place and never start, so it
                // is refused here rather than discovered on a dark installation.
                let base = router_proto::layout::APP_BASE_LEGACY;
                let linked_here = match router_proto::app_image::image_base(&firmware) {
                    Ok((image_base, _)) if image_base != base => {
                        eprintln!(
                            "FW upload refused: image is linked for 0x{image_base:08X}; the \
                             broadcast path only reaches bootloaders whose application is at \
                             0x{base:08X}"
                        );
                        false
                    }
                    Err(error) => {
                        eprintln!("FW upload refused: {error}");
                        false
                    }
                    Ok(_) => true,
                };
                let params = if col.is_some() {
                    crate::fw_update::FwUpdateParams::default()
                } else {
                    crate::fw_update::FwUpdateParams::mass()
                };
                let targets: Vec<usize> = match col {
                    _ if !linked_here => Vec::new(),
                    Some(col) => vec![col],
                    None => installation
                        .columns
                        .iter()
                        .enumerate()
                        .filter(|(_, c)| c.rs485.is_connected())
                        .map(|(i, _)| i)
                        .collect(),
                };
                for col in targets {
                    if let Some(column) = installation.column(col) {
                        match crate::fw_update::upload(&column.rs485, &firmware, base, &params) {
                            Ok(queued) => reporter.emit(router_report::Event::Marker {
                                label: format!(
                                    "FW upload queued: column {col}, {queued} packets, {} bytes",
                                    firmware.len()
                                ),
                            }),
                            Err(error) => eprintln!("FW upload refused: {error}"),
                        }
                    }
                }
            }
            Err(e) => eprintln!("FW upload: cannot read {}: {e}", path.display()),
        },
        FwErase { col } => {
            let params = crate::fw_update::FwUpdateParams::default();
            for_columns(installation, col, |column| {
                crate::fw_update::erase(&column.rs485, &params);
            });
        }
        FwRun { col } => {
            let params = crate::fw_update::FwUpdateParams::default();
            for_columns(installation, col, |column| {
                crate::fw_update::run_application(&column.rs485, &params);
            });
        }
        RepeaterStatus { col, repeater } => {
            if let Some(column) = installation.column(col) {
                // Status answers, so it is unicast per repeater. Asking all six at
                // once would put six replies on the wire together.
                let indices: Vec<u8> = match repeater {
                    Some(index) => vec![index],
                    None => (1..=router_proto::REPEATER_COUNT).collect(),
                };
                for index in indices {
                    repeater_request(
                        &column.rs485,
                        RepeaterTarget::Index(index),
                        RepeaterVerb::Status,
                        None,
                    );
                }
            }
        }
        RepeaterSetIndex { col, mac, index } => {
            if let Some(column) = installation.column(col) {
                // Addressed by MAC: the unit being provisioned has no index yet, or
                // has the wrong one, which is exactly why this exists.
                repeater_request(
                    &column.rs485,
                    RepeaterTarget::Mac(mac),
                    RepeaterVerb::SetIndex,
                    Some(router_proto::Value::from(index)),
                );
            }
        }
        RepeaterRelearn { col, repeater } => {
            if let Some(column) = installation.column(col) {
                repeater_request(
                    &column.rs485,
                    RepeaterTarget::Index(repeater),
                    RepeaterVerb::Relearn,
                    None,
                );
            }
        }
        RepeaterResetCounters { col, repeater } => {
            if let Some(column) = installation.column(col) {
                repeater_request(
                    &column.rs485,
                    RepeaterTarget::Index(repeater),
                    RepeaterVerb::ResetCounters,
                    None,
                );
            }
        }
        RepeaterReboot { col, repeater } => {
            if let Some(column) = installation.column(col) {
                repeater_request(
                    &column.rs485,
                    RepeaterTarget::Index(repeater),
                    RepeaterVerb::Reboot,
                    None,
                );
            }
        }
        RepeaterSnapshot { col } => {
            if let Some(column) = installation.column(col) {
                // One broadcast starts all six branch sweeps in parallel -- the
                // branches are isolated, so they genuinely overlap. The host then
                // reads them back one at a time, staying the sole bus arbiter.
                repeater_request(
                    &column.rs485,
                    RepeaterTarget::All,
                    RepeaterVerb::SnapshotStart,
                    None,
                );
                for index in 1..=router_proto::REPEATER_COUNT {
                    repeater_request(
                        &column.rs485,
                        RepeaterTarget::Index(index),
                        RepeaterVerb::SnapshotRead,
                        None,
                    );
                }
            }
        }
        RepeaterOta { col, repeater, path } => match std::fs::read(&path) {
            Ok(image) => {
                let params = crate::repeater_ota::RepeaterOtaParams::default();
                match crate::repeater_ota::RepeaterImage::new(image, params.chunk_bytes) {
                    Ok(image) => {
                        if let Some(column) = installation.column(col) {
                            let indices: Vec<u8> = match repeater {
                                Some(index) => vec![index],
                                None => (1..=router_proto::REPEATER_COUNT).collect(),
                            };
                            let seconds = image.estimated_seconds(&params);
                            for index in indices {
                                let target = RepeaterTarget::Index(index);
                                // begin is acknowledged, and nothing may be streamed
                                // until it answers: the erase runs with the flash
                                // cache off, so the UART ISR cannot run and inbound
                                // bytes are lost while it does.
                                crate::repeater_ota::begin(
                                    &column.rs485,
                                    &target,
                                    &image,
                                    &params,
                                );
                                crate::repeater_ota::send_chunks(
                                    &column.rs485,
                                    &target,
                                    &image,
                                    &params,
                                    &crate::repeater_ota::all_indices(&image),
                                );
                                // Reads the received-chunk bitmap back. Repair of
                                // whatever it reports missing is driven by the reply
                                // handler, not queued blindly here.
                                crate::repeater_ota::request_map(
                                    &column.rs485,
                                    &target,
                                    &params,
                                );
                                crate::repeater_ota::end(&column.rs485, &target, &params);
                            }
                            reporter.emit(router_report::Event::Marker {
                                label: format!(
                                    "repeater OTA queued: column {col}, {} bytes, \
                                     {} chunks, about {seconds:.0}s per repeater",
                                    image.len(),
                                    image.chunk_count()
                                ),
                            });
                        }
                    }
                    Err(error) => eprintln!("repeater OTA refused: {error}"),
                }
            }
            Err(e) => eprintln!("repeater OTA: cannot read {}: {e}", path.display()),
        },
        RepeaterOtaAbort { col, repeater } => {
            if let Some(column) = installation.column(col) {
                // Releases the paused bridge at once, instead of waiting out the
                // repeater's 30-second inactivity timeout.
                column.rs485.clear_outbox();
                let target = match repeater {
                    Some(index) => RepeaterTarget::Index(index),
                    None => RepeaterTarget::All,
                };
                crate::repeater_ota::abort(&column.rs485, &target);
            }
        }
        SourceAdd { type_name } => {
            if let Some(source) = crate::image::sources::create_by_type_name(&type_name) {
                renderer.add_source(source);
            }
        }
        SourceRemove { index } => renderer.remove_source(index),
        SourceSetParams { index, params } => {
            if let Some(source) = renderer.sources.get_mut(index) {
                source.deserialise(&params);
            }
        }
        ClearPortalLog { col, portal } => {
            if let Some(p) = installation.portal(col, portal) {
                p.logger.messages.clear();
            }
        }
        Marker(label) => reporter.emit(router_report::Event::Marker { label }),
        SaveConfig => {
            if let Some(path) = config_path {
                sync_config(app_config, installation, renderer);
                if let Err(e) = app_config.save(path) {
                    eprintln!("config save failed: {e}");
                }
            }
        }
        Shutdown => return false,
        Query(query) => handle_query(query, installation),
    }
    true
}

fn sync_config(app_config: &mut AppConfig, installation: &Installation, renderer: &Renderer) {
    let inst = &mut app_config.installation;
    inst.arrangement.columns = installation.columns_count;
    inst.arrangement.rows = installation.rows;
    inst.arrangement.column_width = installation.column_width;
    inst.arrangement.flipped = installation.flipped;
    inst.messaging.transmit = installation.transmit;
    inst.messaging.period_s = installation.period_s;
    inst.messaging.keyframe_batch_size = installation.keyframe_batch_size;
    inst.messaging.keyframe_velocities = installation.keyframe_velocities;
    inst.image_enabled = installation.image_enabled;
    // per-column: keep count/flip and live rs485 settings
    for (i, column) in installation.columns.iter().enumerate() {
        if let Some(entry) = inst.columns.get_mut(i) {
            entry.count_x = column.count_x;
            entry.count_y = column.count_y;
            entry.flipped = Some(column.flipped);
            if let Some(settings) = column.rs485.settings() {
                entry.rs485 = Some(settings.clone());
            }
        }
    }
    app_config.renderer_sources = renderer.serialise_sources();
}

/// Queues one control-plane request. Never collateable and never given a collation
/// address: the outbox keeps only the newest packet per (address, target), which
/// would otherwise silently delete all but the last of a chunk stream.
fn repeater_request(
    rs485: &crate::rs485::Rs485,
    target: RepeaterTarget,
    verb: RepeaterVerb,
    payload: Option<router_proto::Value>,
) {
    use crate::rs485::{Packet, Payload};
    let needs_ack = verb.expects_reply();
    rs485.transmit(Packet {
        payload: Payload::Rendered(router_proto::repeater::request(&target, verb, payload)),
        target: target.reply_source().unwrap_or(router_proto::HOST),
        address: String::new(),
        needs_ack,
        collateable: false,
        custom_wait_time_ms: if needs_ack { None } else { Some(0) },
        on_sent: None,
    });
}

fn for_columns(
    installation: &mut Installation,
    col: Option<usize>,
    mut f: impl FnMut(&mut crate::model::column::Column),
) {
    match col {
        Some(col) => {
            if let Some(column) = installation.column(col) {
                f(column);
            }
        }
        None => {
            for column in &mut installation.columns {
                if column.rs485.is_connected() {
                    f(column);
                }
            }
        }
    }
}

fn for_scope(
    installation: &mut Installation,
    scope: Scope,
    mut f: impl FnMut(&mut crate::model::portal::Portal),
) {
    match scope {
        Scope::All => {
            for column in &mut installation.columns {
                for portal in &mut column.portals {
                    f(portal);
                }
            }
        }
        Scope::Column(col) => {
            if let Some(column) = installation.column(col) {
                for portal in &mut column.portals {
                    f(portal);
                }
            }
        }
        Scope::Portal(col, portal) => {
            if let Some(p) = installation.portal(col, portal) {
                f(p);
            }
        }
    }
}

fn handle_query(query: Query, installation: &mut Installation) {
    match query {
        Query::GetPosition { col, portal, reply } => {
            let value = installation
                .portal(col, portal)
                .map(|p| p.pilot.live_position());
            let _ = reply.send(value);
        }
        Query::GetTargetPosition { col, portal, reply } => {
            let value = installation
                .portal(col, portal)
                .map(|p| p.pilot.live_target_position());
            let _ = reply.send(value);
        }
        Query::IsInPosition { col, portal, reply } => {
            let value = installation
                .portal(col, portal)
                .map(|p| p.pilot.is_in_target_position());
            let _ = reply.send(value);
        }
        Query::PortalExists { col, portal, reply } => {
            let _ = reply.send(installation.portal(col, portal).is_some());
        }
    }
}

fn build_snapshot(
    installation: &mut Installation,
    renderer: &Renderer,
    generation: u64,
    servers: &ServerInfo,
    _app_config: &AppConfig,
) -> UiSnapshot {
    let columns = installation
        .columns
        .iter()
        .map(|column| {
            let portals = column
                .portals
                .iter()
                .map(|portal| {
                    let pilot = &portal.pilot;
                    let live_axes_known = pilot.live_axis_known.iter().all(|k| *k);
                    PortalSnapshot {
                        target: portal.target,
                        axes: pilot.axes,
                        polar: pilot.polar,
                        position: pilot.position,
                        live_position: live_axes_known.then(|| pilot.live_position()),
                        live_target_position: pilot
                            .live_target_known
                            .iter()
                            .all(|k| *k)
                            .then(|| pilot.live_target_position()),
                        live_axes: live_axes_known
                            .then(|| vec2(pilot.live_axis.x, pilot.live_axis.y)),
                        in_target_position: pilot.is_in_target_position(),
                        last_rx_age_ms: portal.last_rx.map(|t| t.elapsed().as_millis() as u64),
                        last_tx_age_ms: portal.last_tx.map(|t| t.elapsed().as_millis() as u64),
                        up_time_ms: portal.reported.up_time_ms,
                        version: portal.reported.version.clone(),
                        last_log: portal
                            .logger
                            .last_message()
                            .map(|m| (m.level, m.message.clone(), m.count)),
                        logs: portal
                            .logger
                            .messages
                            .iter()
                            .rev()
                            .take(20)
                            .rev()
                            .map(|m| (m.level, m.message.clone(), m.count))
                            .collect(),
                        offset: pilot.offset,
                        leading_control: match pilot.leading_control {
                            LeadingControl::Position => "Position",
                            LeadingControl::Polar => "Polar",
                            LeadingControl::Axes => "Axes",
                        },
                        mc: [0, 1].map(|i| {
                            let mc = &portal.motion_control[i];
                            snapshot::McSnapshot {
                                reported_position: mc.reported_position,
                                reported_target: mc.reported_target,
                                max_velocity: mc.max_velocity,
                                acceleration: mc.acceleration,
                                min_velocity: mc.min_velocity,
                                health_ok: mc.reported_health.all_ok(),
                            }
                        }),
                        mds_current_amps: portal.motor_driver_settings.current_amps,
                        mds_microstep_resolution: portal.motor_driver_settings.microstep_resolution,
                        poll_regularly: portal.poll_regularly,
                        poll_interval_s: portal.poll_interval_s,
                        send_periodically: pilot.send_periodically,
                    }
                })
                .collect();
            ColumnSnapshot {
                index: column.index,
                count_x: column.count_x,
                count_y: column.count_y,
                panel_height: column.panel_height,
                flipped: column.flipped,
                stats: column.rs485.stats(),
                portals,
                scheduled_poll_enabled: column.scheduled_poll_enabled,
                scheduled_poll_period_s: column.scheduled_poll_period_s,
                repeaters: column.repeaters.records().to_vec(),
            }
        })
        .collect();

    UiSnapshot {
        generation,
        resolution: installation.resolution(),
        columns,
        preview: renderer.pixels.clone(),
        image_enabled: installation.image_enabled,
        arrangement: (
            installation.columns_count,
            installation.rows,
            installation.column_width,
            installation.panel_height,
            installation.flipped,
        ),
        period_s: installation.period_s,
        keyframe_batch_size: installation.keyframe_batch_size,
        keyframe_velocities: installation.keyframe_velocities,
        transmit_mode: match installation.transmit {
            crate::config::ImageTransmit::Individual => "Individual",
            crate::config::ImageTransmit::Keyframe => "Keyframe",
            crate::config::ImageTransmit::Disabled => "Disabled",
        },
        osc_running: servers.osc_running,
        osc_port: servers.osc_port,
        rest_running: servers.rest_running,
        rest_port: servers.rest_port,
        osc_messages_per_tick: servers.osc_message_count.swap(0, Ordering::Relaxed),
        sources: renderer.serialise_sources(),
    }
}
