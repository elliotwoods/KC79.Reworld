//! The one thread that owns the hardware.
//!
//! It holds the [`Bench`], ticks it, and mirrors the result into bus parameters and into the
//! shared snapshot the HTTP routes read. Nothing else in the process touches a link — see the
//! transport module docs in `bench-core` for why that is a correctness property.
//!
//! # Two front doors, one queue
//!
//! Requests arrive from the page as **counter bumps** on `/actions/*`, and from an agent as
//! HTTP posts that push onto [`Shared::requests`]. Both are drained here, in one place, in
//! arrival order. That is what makes "a human watching a plot while an agent drives" a
//! supported mode rather than a race.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use av_gui_bus::{Bus, ParamId, Value};
use bench_core::bench::{Bench, Origin, Outcome, RunStatus};
use bench_core::dut::{Axis, FirmwareKind, GearRatio};
use bench_core::engine::Phase;
use bench_core::plan::Plan;
use bench_core::state::BenchState;
use bench_core::transport::{Channel, LineEnding, LinkKind, MotionProfile, Op, RawSignal};
use portal_swd::Pass;

use crate::flash::{FlashController, FlashSnapshot};
use crate::schema::{self, AxisParams, Params};

/// Worker tick period.
///
/// 20 ms is fast enough that a 60 Hz status stream from the bench firmware is read without the
/// serial buffer growing, and slow enough that an idle bench costs nothing.
const TICK: Duration = Duration::from_millis(20);

/// Something an agent asked for over HTTP.
#[derive(Debug, Clone)]
pub enum Request {
    Connect { kind: LinkKind, endpoint: String },
    Disconnect,
    DisconnectChannel(Channel),
    Submit(Op),
    SubmitTo { channel: Channel, op: Op },
    SelectRs485Target(i8),
    DiscoverRs485,
    SelectChannel(Channel),
    Run { plan: Box<Plan>, origin: Origin },
    FlashNow,
    ResetMcu,
    CheckBoot,
    ReadDevice,
    RescanFirmware,
    SendRaw { channel: Channel, signal: RawSignal },
    Abort,
}

/// What the HTTP routes read and write. The worker owns the hardware; this is the mirror.
#[derive(Default)]
pub struct Shared {
    pub state: Mutex<BenchState>,
    pub run: Mutex<Option<RunStatus>>,
    pub last: Mutex<Option<Outcome>>,
    /// Log lines, already rendered, with their cursor.
    pub log: Mutex<Vec<serde_json::Value>>,
    /// Bounded raw motion samples for canvas consumers and agents.
    pub telemetry: Mutex<Vec<serde_json::Value>>,
    /// SWD/probe state and the production artefact catalogue.
    pub flash: Mutex<FlashSnapshot>,
    pub artefacts: Mutex<serde_json::Value>,
    /// Requests waiting to be drained by the worker.
    pub requests: Mutex<Vec<Request>>,
    /// Filled in by the worker when a `Run` request could not start.
    pub last_start_error: Mutex<Option<String>>,
}

impl Shared {
    pub fn push(&self, request: Request) {
        self.requests.lock().unwrap().push(request);
    }
}

pub struct Worker {
    bus: Arc<Bus>,
    params: Params,
    bench: Bench,
    shared: Arc<Shared>,
    started: Instant,
    /// Last seen value of each action counter, so only a *change* fires.
    action_seen: Vec<i64>,
    plans_dir: std::path::PathBuf,
    cue_seq: i64,
    last_seq: i64,
    log_cursor: u64,
    telemetry_cursor: u64,
    flash: FlashController,
    flash_selection_seen: (String, String),
    probe_selection_seen: String,
    flash_heartbeat_seen: i64,
    flash_was_armed: bool,
    /// A successful SWD flash should hand straight over to the probe's VCOM. Windows can take a
    /// moment to enumerate that interface after reset, so pairing is retried without blocking the
    /// worker or ever falling back to an unrelated COM port.
    auto_vcom: Option<AutoVcom>,
    sim_module_present: Option<ParamId>,
    /// Last heartbeat value seen from the page, and when we saw it change.
    heartbeat: (i64, u64),
}

/// How long a page may go quiet before it counts as gone.
///
/// The page beats once a second, so five seconds is several missed beats rather than one
/// unlucky scheduling gap.
const HEARTBEAT_STALE_MS: u64 = 5_000;
const VCOM_ATTACH_TIMEOUT_MS: u64 = 5_000;
const VCOM_ATTACH_RETRY_MS: u64 = 250;

struct AutoVcom {
    deadline_ms: u64,
    next_attempt_ms: u64,
    last_error: String,
}

impl Worker {
    pub fn new(
        bus: Arc<Bus>,
        params: Params,
        bench: Bench,
        shared: Arc<Shared>,
        plans_dir: std::path::PathBuf,
        simulated: bool,
    ) -> Self {
        let action_count = params.actions.len();
        let sim_module_present = bus.id_of("/sim/module_present");
        let flash = FlashController::new(simulated);
        let initial = flash.snapshot();
        let _ = bus.set_text(params.flash.boot_id, &initial.boot_id);
        let _ = bus.set_text(params.flash.app_id, &initial.app_id);
        let _ = bus.set_text(params.probe.selected, flash.probe_selector());
        *shared.flash.lock().unwrap() = initial.clone();
        *shared.artefacts.lock().unwrap() = flash.artefacts_json();
        Self {
            bus,
            params,
            bench,
            shared,
            started: Instant::now(),
            // Adopt whatever the counters currently read rather than zero: a worker that
            // restarts must not replay every action the page has ever taken.
            action_seen: vec![i64::MIN; action_count],
            plans_dir,
            cue_seq: 0,
            last_seq: 0,
            log_cursor: 0,
            telemetry_cursor: 0,
            flash_selection_seen: (initial.boot_id.clone(), initial.app_id.clone()),
            probe_selection_seen: flash.probe_selector().to_string(),
            flash_heartbeat_seen: 0,
            flash_was_armed: false,
            auto_vcom: None,
            sim_module_present,
            flash,
            heartbeat: (0, 0),
        }
    }

    /// Whether the page has gone quiet.
    ///
    /// **This is deliberately narrower than PortalFlasher's dead-man.** There, a stale heartbeat
    /// disarms the rig, because an unattended flasher with a pogo fixture is dangerous. Here it
    /// only blocks *starting* destructive work from the page; it never cancels a run in flight,
    /// because closing a browser tab must not abort an eight-hour soak. And it never gates the
    /// HTTP door at all: an agent driving `ptb` has no page and is not supposed to need one.
    fn page_is_stale(&mut self, now: u64) -> bool {
        let value = schema::get_i64(&self.bus, self.params.ui_heartbeat);
        if value != self.heartbeat.0 {
            self.heartbeat = (value, now);
        }
        // Nothing has ever beaten: the page has not connected yet, which is not the same as one
        // that connected and stopped. Treat it as stale either way -- both mean "nobody is
        // watching" -- but only after the same grace period, so a fresh start is not blocked.
        now.saturating_sub(self.heartbeat.1) > HEARTBEAT_STALE_MS
    }

    fn now_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    pub fn run(mut self) {
        // First pass only reads the counters, so nothing fires on startup.
        for (index, (_, id)) in self.params.actions.iter().enumerate() {
            self.action_seen[index] = schema::get_i64(&self.bus, *id);
        }

        loop {
            let now = self.now_ms();
            self.handle_actions(now);
            self.handle_requests(now);

            self.sync_flash_selection(now);
            self.sync_probe_selection(now);
            if let Some(id) = self.sim_module_present {
                self.flash.set_sim_present(schema::get_bool(&self.bus, id));
            }
            // Feed the flasher state machine only real browser beats. Re-sending "alive" on
            // every 20 ms worker tick would extend its five-second dead-man to ten seconds.
            let heartbeat = schema::get_i64(&self.bus, self.params.ui_heartbeat);
            let page_heartbeat = heartbeat != self.flash_heartbeat_seen;
            if page_heartbeat {
                self.flash_heartbeat_seen = heartbeat;
            }
            let requested_auto = schema::get_bool(&self.bus, self.params.flash.auto_enabled);
            let auto_enabled = requested_auto && !self.bench.is_busy();
            if requested_auto && self.bench.is_busy() {
                let _ = self
                    .bus
                    .set(self.params.flash.auto_enabled, Value::Bool(false));
                self.bench.note(
                    now,
                    bench_core::LOG_LEVEL_WARNING,
                    "auto-flash refused while a test plan is running",
                );
            }
            if let Some(pass) = self.flash.tick(now, page_heartbeat, auto_enabled) {
                self.run_flash(now, pass, true);
            }
            let armed = self.flash.snapshot().armed;
            if self.flash_was_armed && !armed && auto_enabled {
                // A safety/failure transition owns the observed truth. Retract the desired
                // toggle or the page would still claim automatic flashing is enabled.
                let _ = self
                    .bus
                    .set(self.params.flash.auto_enabled, Value::Bool(false));
            }
            self.flash_was_armed = armed;

            let post_io_now = self.now_ms();
            self.tick_auto_vcom(post_io_now);
            if let Some(outcome) = self.bench.tick(post_io_now) {
                self.publish_outcome(&outcome);
                *self.shared.last.lock().unwrap() = Some(outcome);
            }

            self.publish(post_io_now);
            std::thread::sleep(TICK);
        }
    }

    /// Turn counter bumps from the page into work.
    fn handle_actions(&mut self, now: u64) {
        for index in 0..self.params.actions.len() {
            let (name, id) = self.params.actions[index];
            let value = schema::get_i64(&self.bus, id);
            if value == self.action_seen[index] {
                continue;
            }
            self.action_seen[index] = value;
            self.act(name, now);
        }
    }

    fn act(&mut self, name: &str, now: u64) {
        match name {
            "connect_serial" => self.connect_from(Channel::Serial, now),
            "disconnect_serial" => self.bench.disconnect_channel(Channel::Serial, now),
            "identify_serial" => self.bench.submit_to(Channel::Serial, Op::Identify),
            "connect_rs485" => self.connect_from(Channel::Rs485, now),
            "disconnect_rs485" => self.bench.disconnect_channel(Channel::Rs485, now),
            "discover_rs485" => self.bench.discover_rs485(),
            "identify_rs485" => self.bench.submit_to(Channel::Rs485, Op::Identify),
            "select_rs485_target" => {
                let target = schema::get_i32(&self.bus, self.params.rs485_target) as i8;
                if let Err(error) = self.bench.select_rs485_target(target) {
                    self.bench.note(now, bench_core::LOG_LEVEL_ERROR, error);
                } else {
                    self.bench.submit_to(Channel::Rs485, Op::Identify);
                }
            }
            "connect" => {
                let kind =
                    transport_from(schema::get_u32(&self.bus, self.params.transport_desired));
                let endpoint = schema::get_text(&self.bus, self.params.transport_port);
                match kind {
                    Some(kind) => {
                        if let Err(error) = self.bench.connect(kind, &endpoint, now) {
                            let _ = self.bus.set_text(self.params.transport_detail, &error);
                        }
                        self.cue(if self.bench.state().link.connected {
                            "connected"
                        } else {
                            "lost"
                        });
                    }
                    None => self.bench.note(
                        now,
                        bench_core::LOG_LEVEL_WARNING,
                        "pick a transport first",
                    ),
                }
            }
            "disconnect" => {
                self.bench.disconnect(now);
                self.cue("lost");
            }
            "identify" => self.submit_test(now, Op::Identify),
            "poll" => self.submit_test(now, Op::Poll),
            "escape" => self.submit_direct(Op::Escape),
            "calibrate_threshold" => self.submit_test(now, Op::Calibrate { axis: Axis::A }),
            "calibrate_module" => self.submit_test(now, Op::Calibrate { axis: Axis::A }),
            "routine_startup" => self.submit_test(now, Op::Startup),
            "home_a" => self.submit_test(now, Op::Home { axis: Axis::A }),
            "home_b" => self.submit_test(now, Op::Home { axis: Axis::B }),
            "unjam_a" => self.submit_test(now, Op::Unjam { axis: Axis::A }),
            "unjam_b" => self.submit_test(now, Op::Unjam { axis: Axis::B }),
            "backlash_a" => self.submit_test(now, Op::MeasureBacklash { axis: Axis::A }),
            "backlash_b" => self.submit_test(now, Op::MeasureBacklash { axis: Axis::B }),
            "reboot" => self.submit_test(now, Op::Reboot),
            "set_current" => self.submit_test(
                now,
                Op::SetCurrent {
                    amps: schema::get_f64(&self.bus, self.params.test.current_a) as f32,
                },
            ),
            "set_microstep" => self.submit_test(
                now,
                Op::SetMicrostep {
                    resolution: schema::get_i32(&self.bus, self.params.test.microstep).max(1)
                        as u32,
                },
            ),
            "set_threshold" => self.submit_test(
                now,
                Op::SetHomeThreshold {
                    value: schema::get_i32(&self.bus, self.params.test.home_threshold),
                },
            ),
            "census_a" => self.run_census(now, Axis::A),
            "census_b" => self.run_census(now, Axis::B),
            "send_raw" => self.send_raw(now),
            "abort" => {
                if self.bench.abort(now) {
                    self.cue("abort");
                }
            }
            "marker" => self
                .bench
                .note(now, bench_core::LOG_LEVEL_STATUS, "operator marker"),
            "run" | "startup" => {
                if self.page_is_stale(now) {
                    // The press arrived, so a page exists -- but it has not beaten in five
                    // seconds, which means the bus is not carrying its writes. Starting a
                    // minutes-long routine that nobody can then see or abort is worse than
                    // refusing it.
                    self.bench.note(
                        now,
                        bench_core::LOG_LEVEL_WARNING,
                        "refused: this page has lost contact with the bench",
                    );
                    self.cue("attention");
                    return;
                }
                let name = if name == "startup" {
                    "startup".to_string()
                } else {
                    let selected = schema::get_text(&self.bus, self.params.plan_selected);
                    if selected.is_empty() {
                        "startup".to_string()
                    } else {
                        selected
                    }
                };
                self.bench.select_channel(self.motion_channel());
                self.start_named(&name, Origin::Gui, now);
            }
            "motion_push" => self.push_motion(now),
            "flash" | "flash_now" => {
                if self.page_is_stale(now) {
                    self.bench.note(
                        now,
                        bench_core::LOG_LEVEL_WARNING,
                        "refused flash: page heartbeat is stale",
                    );
                } else if self.bench.is_busy() {
                    self.bench.note(
                        now,
                        bench_core::LOG_LEVEL_WARNING,
                        "refused flash: a test plan is running",
                    );
                } else if let Err(error) = self.flash.manual_ready() {
                    self.bench.note(now, bench_core::LOG_LEVEL_WARNING, error);
                } else {
                    self.run_flash(now, Pass::Flash, false);
                }
            }
            "reset_mcu" => self.reset_mcu(now, true),
            "check_boot" => self.check_mcu_boot(now),
            "read_device" => self.flash.read_device(),
            "rescan_firmware" => self.rescan_setup(now),
            "rescan" => {
                self.rescan_setup(now);
                let survey = bench_core::survey();
                self.bench.note(
                    now,
                    bench_core::LOG_LEVEL_STATUS,
                    format!(
                        "rescan: {} serial ports, {} debug probes",
                        survey.ports.len(),
                        survey.probes.len()
                    ),
                );
            }
            other => self.bench.note(
                now,
                bench_core::LOG_LEVEL_WARNING,
                format!("`{other}` is not wired up yet"),
            ),
        }
    }

    fn motion_channel(&self) -> Channel {
        match schema::get_u32(&self.bus, self.params.motion.route) {
            1 => Channel::Rs485,
            _ => Channel::Serial,
        }
    }

    fn submit_direct(&mut self, op: Op) {
        self.bench.submit_to(self.motion_channel(), op);
    }

    fn submit_test(&mut self, now: u64, op: Op) {
        if self.flash.snapshot().armed || self.flash.snapshot().busy || self.bench.is_busy() {
            self.bench.note(
                now,
                bench_core::LOG_LEVEL_WARNING,
                "test command refused while the fixture is active",
            );
            return;
        }
        self.bench.submit_to(self.motion_channel(), op);
    }

    fn run_census(&mut self, now: u64, axis: Axis) {
        self.submit_test(
            now,
            Op::Census {
                axis,
                threshold: schema::get_i32(&self.bus, self.params.test.home_threshold).clamp(0, 255)
                    as u8,
                speed: match schema::get_i32(&self.bus, self.params.test.census_speed) {
                    0 => None,
                    speed => Some(speed),
                },
            },
        );
    }

    fn send_raw(&mut self, now: u64) {
        if self.flash.snapshot().armed || self.flash.snapshot().busy || self.bench.is_busy() {
            self.bench.note(
                now,
                bench_core::LOG_LEVEL_WARNING,
                "raw signal refused while the fixture is active",
            );
            return;
        }
        let channel = self.motion_channel();
        let signal = match channel {
            Channel::Serial => RawSignal::VcomText {
                text: schema::get_text(&self.bus, self.params.test.raw_vcom_text),
                ending: match schema::get_u32(&self.bus, self.params.test.raw_line_ending) {
                    1 => LineEnding::Cr,
                    2 => LineEnding::Lf,
                    3 => LineEnding::Crlf,
                    _ => LineEnding::None,
                },
            },
            Channel::Rs485 => {
                let text = schema::get_text(&self.bus, self.params.test.raw_rs485_json);
                let body: serde_json::Value = match serde_json::from_str(&text) {
                    Ok(serde_json::Value::Object(entries)) => serde_json::Value::Object(entries),
                    Ok(_) => {
                        self.bench.note(
                            now,
                            bench_core::LOG_LEVEL_ERROR,
                            "raw RS485 payload must be a JSON object",
                        );
                        return;
                    }
                    Err(error) => {
                        self.bench.note(
                            now,
                            bench_core::LOG_LEVEL_ERROR,
                            format!("raw RS485 JSON is invalid: {error}"),
                        );
                        return;
                    }
                };
                RawSignal::Rs485Json { body }
            }
        };
        self.bench.submit_raw(channel, signal, now);
    }

    fn connect_from(&mut self, channel: Channel, now: u64) {
        let (kind, endpoint) = match channel {
            Channel::Serial => (
                match schema::get_u32(&self.bus, self.params.serial.desired) {
                    1 => Some(LinkKind::Vcp),
                    2 => Some(LinkKind::BenchAscii),
                    _ => None,
                },
                schema::get_text(&self.bus, self.params.serial.endpoint),
            ),
            Channel::Rs485 => (
                match schema::get_u32(&self.bus, self.params.rs485.desired) {
                    1 => Some(LinkKind::Rs485Serial),
                    2 => Some(LinkKind::Rs485Tcp),
                    _ => None,
                },
                schema::get_text(&self.bus, self.params.rs485.endpoint),
            ),
        };
        let Some(kind) = kind else {
            self.bench.note(
                now,
                bench_core::LOG_LEVEL_WARNING,
                format!("pick a {} transport first", channel.name()),
            );
            return;
        };
        if let Err(error) = self.bench.connect(kind, &endpoint, now) {
            let param = match channel {
                Channel::Serial => self.params.serial.detail,
                Channel::Rs485 => self.params.rs485.detail,
            };
            let _ = self.bus.set_text(param, &error);
            self.cue("lost");
        } else {
            if channel == Channel::Rs485 {
                let target = schema::get_i32(&self.bus, self.params.rs485_target) as i8;
                let _ = self.bench.select_rs485_target(target);
            }
            self.cue("connected");
        }
    }

    fn schedule_auto_vcom(&mut self, now: u64) {
        if self.flash.probe_selector() == "sim" {
            return;
        }
        self.bench.note(
            now,
            bench_core::LOG_LEVEL_STATUS,
            "flash complete; attaching the selected probe's VCOM",
        );
        self.auto_vcom = Some(AutoVcom {
            deadline_ms: now.saturating_add(VCOM_ATTACH_TIMEOUT_MS),
            next_attempt_ms: now,
            last_error: "VCOM has not enumerated yet".into(),
        });
        self.tick_auto_vcom(now);
    }

    fn tick_auto_vcom(&mut self, now: u64) {
        if self.bench.state().channels.serial.link.connected {
            self.auto_vcom = None;
            return;
        }
        let Some(pending) = self.auto_vcom.as_ref() else {
            return;
        };
        if now < pending.next_attempt_ms {
            return;
        }

        let survey = bench_core::survey();
        match bench_core::survey::paired_vcom_port(&survey, self.flash.probe_selector()) {
            Ok(endpoint) => {
                self.auto_vcom = None;
                let _ = self.bus.set(
                    self.params.serial.desired,
                    Value::Enum(serial_transport_value(Some(LinkKind::Vcp))),
                );
                let _ = self.bus.set_text(self.params.serial.endpoint, &endpoint);
                if self.bench.connect(LinkKind::Vcp, &endpoint, now).is_ok() {
                    self.bench.note(
                        now,
                        bench_core::LOG_LEVEL_STATUS,
                        format!("VCOM auto-attached on {endpoint}; following firmware output"),
                    );
                    self.cue("connected");
                } else {
                    self.cue("lost");
                }
            }
            Err(error) if now >= pending.deadline_ms => {
                let detail = if error == pending.last_error {
                    error
                } else {
                    format!("{error}; previous check: {}", pending.last_error)
                };
                self.auto_vcom = None;
                self.bench.note(
                    now,
                    bench_core::LOG_LEVEL_WARNING,
                    format!("flash passed, but VCOM could not be auto-attached: {detail}"),
                );
            }
            Err(error) => {
                if let Some(pending) = self.auto_vcom.as_mut() {
                    pending.next_attempt_ms = now.saturating_add(VCOM_ATTACH_RETRY_MS);
                    pending.last_error = error;
                }
            }
        }
    }

    fn push_motion(&mut self, now: u64) {
        if self.flash.snapshot().armed || self.flash.snapshot().busy || self.bench.is_busy() {
            self.bench.note(
                now,
                bench_core::LOG_LEVEL_WARNING,
                "motion command refused while the fixture is active",
            );
            return;
        }
        let channel = self.motion_channel();
        self.bench.select_channel(channel);
        let usteps = self.bench.state().dut.usteps_per_rev;
        let Some(usteps) = usteps else {
            self.bench.note(
                now,
                bench_core::LOG_LEVEL_WARNING,
                "identify or home the module before commanding rotations",
            );
            return;
        };
        let to_steps = |rotations: f64, invert: f64| -> i32 {
            (rotations * f64::from(usteps) * invert)
                .round()
                .clamp(i32::MIN as f64, i32::MAX as f64) as i32
        };
        let a = to_steps(
            schema::get_f64(&self.bus, self.params.motion.a_rotations),
            1.0,
        );
        let b = to_steps(
            schema::get_f64(&self.bus, self.params.motion.b_rotations),
            -1.0,
        );
        let profile = MotionProfile {
            max_velocity: schema::get_i32(&self.bus, self.params.motion.max_velocity),
            acceleration: schema::get_i32(&self.bus, self.params.motion.acceleration),
            min_velocity: schema::get_i32(&self.bus, self.params.motion.min_velocity),
        };
        if profile.max_velocity > 28_000 {
            self.bench.note(
                now,
                bench_core::LOG_LEVEL_ERROR,
                "refused pilot move above the 28,000 µsteps/s stall guard",
            );
            return;
        }
        match channel {
            Channel::Rs485 => {
                self.bench.submit_to(
                    channel,
                    Op::SetMotionProfile {
                        axis: Axis::A,
                        profile,
                    },
                );
                self.bench.submit_to(
                    channel,
                    Op::SetMotionProfile {
                        axis: Axis::B,
                        profile,
                    },
                );
                self.bench.submit_to(channel, Op::MoveAxes { a, b });
            }
            Channel::Serial => {
                self.bench.submit_to(
                    channel,
                    Op::MoveTo {
                        axis: Axis::A,
                        usteps: a,
                        profile: Some(profile),
                    },
                );
                if b != 0 {
                    self.bench.note(
                        now,
                        bench_core::LOG_LEVEL_WARNING,
                        "bench serial controls only axis A; axis B was not sent",
                    );
                }
            }
        }
    }

    fn setup_is_locked(&self) -> bool {
        let flash = self.flash.snapshot();
        flash.armed || flash.busy || self.bench.is_busy()
    }

    fn sync_flash_selection(&mut self, now: u64) {
        let selection = (
            schema::get_text(&self.bus, self.params.flash.boot_id),
            schema::get_text(&self.bus, self.params.flash.app_id),
        );
        if selection != self.flash_selection_seen {
            if self.setup_is_locked() {
                let _ = self
                    .bus
                    .set_text(self.params.flash.boot_id, &self.flash_selection_seen.0);
                let _ = self
                    .bus
                    .set_text(self.params.flash.app_id, &self.flash_selection_seen.1);
                self.bench.note(
                    now,
                    bench_core::LOG_LEVEL_WARNING,
                    "firmware selection is locked while the fixture is active",
                );
                return;
            }
            self.flash.select(selection.0.clone(), selection.1.clone());
            self.flash_selection_seen = selection;
            *self.shared.artefacts.lock().unwrap() = self.flash.artefacts_json();
        }
    }

    fn sync_probe_selection(&mut self, now: u64) {
        let selected = schema::get_text(&self.bus, self.params.probe.selected);
        if selected != self.probe_selection_seen {
            if self.setup_is_locked() {
                let _ = self
                    .bus
                    .set_text(self.params.probe.selected, &self.probe_selection_seen);
                self.bench.note(
                    now,
                    bench_core::LOG_LEVEL_WARNING,
                    "probe selection is locked while the fixture is active",
                );
                return;
            }
            self.flash.select_probe(selected);
            self.probe_selection_seen = self.flash.probe_selector().to_string();
            let _ = self
                .bus
                .set_text(self.params.probe.selected, &self.probe_selection_seen);
        }
    }

    fn rescan_setup(&mut self, now: u64) {
        if self.setup_is_locked() {
            self.bench.note(
                now,
                bench_core::LOG_LEVEL_WARNING,
                "fixture rescan is locked while the fixture is active",
            );
            return;
        }
        self.flash.rescan();
        *self.shared.artefacts.lock().unwrap() = self.flash.artefacts_json();
        self.flash_selection_seen = (
            self.flash.snapshot().boot_id.clone(),
            self.flash.snapshot().app_id.clone(),
        );
        self.probe_selection_seen = self.flash.probe_selector().to_string();
        let _ = self
            .bus
            .set_text(self.params.flash.boot_id, &self.flash_selection_seen.0);
        let _ = self
            .bus
            .set_text(self.params.flash.app_id, &self.flash_selection_seen.1);
        let _ = self
            .bus
            .set_text(self.params.probe.selected, &self.probe_selection_seen);
    }

    /// SWD owns reset while a pass runs. Save both communication lanes, close them, and restore
    /// them afterwards so serial and RS485 readers can never race the probe over a reboot.
    fn run_flash(&mut self, now: u64, pass: Pass, automatic: bool) {
        let before = self.bench.state().clone();
        let serial = before
            .channels
            .serial
            .link
            .kind
            .zip(before.channels.serial.link.endpoint.clone());
        let rs485 = before
            .channels
            .rs485
            .link
            .kind
            .zip(before.channels.rs485.link.endpoint.clone());
        let target = before.channels.rs485.selected_target;
        self.bench.disconnect(now);

        let _ = self.bus.set(self.params.flash.busy, Value::Bool(true));
        let _ = self
            .bus
            .set_text(self.params.flash.phase, &pass.to_string());
        let _ = self.bus.set(self.params.flash.progress, Value::F64(0.0));
        let mut in_flight = self.flash.snapshot().clone();
        in_flight.busy = true;
        in_flight.phase = pass.to_string();
        in_flight.progress = 0.0;
        *self.shared.flash.lock().unwrap() = in_flight;

        let bus = Arc::clone(&self.bus);
        let step_id = self.params.flash.step;
        let progress_id = self.params.flash.progress;
        let mut progress = move |step: &str, fraction: f64| {
            let _ = bus.set_text(step_id, step);
            let _ = bus.set(progress_id, Value::F64(fraction.clamp(0.0, 1.0)));
        };
        let ok = self.flash.execute(now, pass, automatic, &mut progress);
        let needs_replug = self.flash.snapshot().needs_replug;
        self.bench.note(
            now,
            if needs_replug {
                bench_core::LOG_LEVEL_WARNING
            } else if ok {
                bench_core::LOG_LEVEL_STATUS
            } else {
                bench_core::LOG_LEVEL_ERROR
            },
            self.flash.snapshot().last_outcome.clone(),
        );

        if let Some((kind, endpoint)) = serial {
            let _ = self.bench.connect(kind, &endpoint, now);
        } else if pass == Pass::Flash && ok && !needs_replug {
            self.schedule_auto_vcom(self.now_ms());
        }
        if let Some((kind, endpoint)) = rs485 {
            let _ = self.bench.connect(kind, &endpoint, now);
            if let Some(target) = target {
                let _ = self.bench.select_rs485_target(target);
            }
        }
        *self.shared.flash.lock().unwrap() = self.flash.snapshot().clone();
    }

    fn reset_mcu(&mut self, now: u64, require_page_heartbeat: bool) {
        if require_page_heartbeat && self.page_is_stale(now) {
            self.bench.note(
                now,
                bench_core::LOG_LEVEL_WARNING,
                "refused MCU reset: page heartbeat is stale",
            );
            return;
        }
        if self.bench.is_busy() {
            self.bench.note(
                now,
                bench_core::LOG_LEVEL_WARNING,
                "refused MCU reset: a test plan is running",
            );
            return;
        }
        if let Err(error) = self.flash.swd_ready() {
            self.bench.note(now, bench_core::LOG_LEVEL_WARNING, error);
            return;
        }

        // A reset invalidates both byte streams. Close them before SWD asserts reset, then restore
        // the operator's two independent connections afterwards.
        let before = self.bench.state().clone();
        let serial = before
            .channels
            .serial
            .link
            .kind
            .zip(before.channels.serial.link.endpoint.clone());
        let rs485 = before
            .channels
            .rs485
            .link
            .kind
            .zip(before.channels.rs485.link.endpoint.clone());
        let target = before.channels.rs485.selected_target;
        self.bench.disconnect(now);

        let result = self.flash.reset_and_run();
        self.bench.note(
            now,
            if result.is_ok() {
                bench_core::LOG_LEVEL_STATUS
            } else {
                bench_core::LOG_LEVEL_ERROR
            },
            self.flash.snapshot().last_outcome.clone(),
        );

        if let Some((kind, endpoint)) = serial {
            let _ = self.bench.connect(kind, &endpoint, now);
        }
        if let Some((kind, endpoint)) = rs485 {
            let _ = self.bench.connect(kind, &endpoint, now);
            if let Some(target) = target {
                let _ = self.bench.select_rs485_target(target);
            }
        }
        *self.shared.flash.lock().unwrap() = self.flash.snapshot().clone();
    }

    fn check_mcu_boot(&mut self, now: u64) {
        if self.bench.is_busy() {
            self.bench.note(
                now,
                bench_core::LOG_LEVEL_WARNING,
                "refused boot check: a test plan is running",
            );
            return;
        }
        if let Err(error) = self.flash.swd_ready() {
            self.bench.note(now, bench_core::LOG_LEVEL_WARNING, error);
            return;
        }
        let result = self.flash.check_boot();
        self.bench.note(
            now,
            if result.is_ok() {
                bench_core::LOG_LEVEL_STATUS
            } else {
                bench_core::LOG_LEVEL_ERROR
            },
            self.flash.snapshot().last_outcome.clone(),
        );
        *self.shared.flash.lock().unwrap() = self.flash.snapshot().clone();
    }

    fn start_named(&mut self, name: &str, origin: Origin, now: u64) {
        let path = self.plans_dir.join(format!("{name}.toml"));
        match bench_core::plan::load(&path) {
            Ok(plan) => self.start(plan, origin, now),
            Err(error) => {
                *self.shared.last_start_error.lock().unwrap() = Some(error.clone());
                self.bench.note(now, bench_core::LOG_LEVEL_ERROR, error);
            }
        }
    }

    fn start(&mut self, plan: Plan, origin: Origin, now: u64) {
        match self.bench.start(plan, origin, now) {
            Ok(_) => {
                *self.shared.last_start_error.lock().unwrap() = None;
                self.cue("run-start");
            }
            Err(error) => {
                let detail = error.to_string();
                *self.shared.last_start_error.lock().unwrap() = Some(detail.clone());
                // A refused plan is a result the operator needs to see, not a silent no-op.
                self.bench.note(now, bench_core::LOG_LEVEL_ERROR, detail);
                self.cue("attention");
            }
        }
    }

    fn handle_requests(&mut self, now: u64) {
        let requests: Vec<Request> = std::mem::take(&mut *self.shared.requests.lock().unwrap());
        for request in requests {
            match request {
                Request::Connect { kind, endpoint } => {
                    let _ = self.bench.connect(kind, &endpoint, now);
                    // Reflect an agent-initiated connect back into the page controls. Without
                    // this the Link selector still reads "none" while the bench is plainly
                    // connected -- the page would be describing an intention nobody holds.
                    let _ = self.bus.set(
                        self.params.transport_desired,
                        Value::Enum(transport_value(kind)),
                    );
                    let _ = self.bus.set_text(self.params.transport_port, &endpoint);
                }
                Request::Disconnect => self.bench.disconnect(now),
                Request::DisconnectChannel(channel) => self.bench.disconnect_channel(channel, now),
                Request::Submit(op) => self.bench.submit(op),
                Request::SubmitTo { channel, op } => self.bench.submit_to(channel, op),
                Request::SelectRs485Target(target) => {
                    if let Err(error) = self.bench.select_rs485_target(target) {
                        self.bench.note(now, bench_core::LOG_LEVEL_ERROR, error);
                    }
                }
                Request::DiscoverRs485 => self.bench.discover_rs485(),
                Request::SelectChannel(channel) => self.bench.select_channel(channel),
                Request::Run { plan, origin } => self.start(*plan, origin, now),
                Request::FlashNow => {
                    if self.bench.is_busy() {
                        self.bench.note(
                            now,
                            bench_core::LOG_LEVEL_WARNING,
                            "refused flash: a test plan is running",
                        );
                    } else if let Err(error) = self.flash.manual_ready() {
                        self.bench.note(now, bench_core::LOG_LEVEL_WARNING, error);
                    } else {
                        self.run_flash(now, Pass::Flash, false);
                    }
                }
                Request::ResetMcu => self.reset_mcu(now, false),
                Request::CheckBoot => self.check_mcu_boot(now),
                Request::ReadDevice => self.flash.read_device(),
                Request::SendRaw { channel, signal } => {
                    if self.flash.snapshot().armed
                        || self.flash.snapshot().busy
                        || self.bench.is_busy()
                    {
                        self.bench.note(
                            now,
                            bench_core::LOG_LEVEL_WARNING,
                            "raw signal refused while the fixture is active",
                        );
                    } else {
                        self.bench.submit_raw(channel, signal, now);
                    }
                }
                Request::RescanFirmware => {
                    self.rescan_setup(now);
                }
                Request::Abort => {
                    self.bench.abort(now);
                }
            }
        }
    }

    fn cue(&mut self, name: &str) {
        let value = schema::CUES
            .iter()
            .find(|(_, n)| *n == name)
            .map(|(v, _)| *v)
            .unwrap_or(0);
        self.cue_seq += 1;
        let _ = self.bus.set(self.params.cue, Value::Enum(value));
        let _ = self.bus.set(self.params.cue_seq, Value::I64(self.cue_seq));
    }

    /// Mirror everything the bench knows into the bus and the shared snapshot.
    fn publish(&mut self, now: u64) {
        let state = self.bench.state().clone();
        let set = |id, value| {
            let _ = self.bus.set(id, value);
        };
        let text = |id, value: &str| {
            let _ = self.bus.set_text(id, value);
        };

        set(
            self.params.transport_connected,
            Value::Bool(state.link.connected),
        );
        set(
            self.params.transport_observed,
            Value::Enum(state.link.kind.map(transport_value).unwrap_or(0)),
        );
        text(
            self.params.transport_detail,
            state.link.detail.as_deref().unwrap_or(""),
        );

        publish_link(
            &self.bus,
            &self.params.serial,
            &state.channels.serial,
            serial_transport_value,
        );
        publish_link(
            &self.bus,
            &self.params.rs485,
            &state.channels.rs485,
            rs485_transport_value,
        );
        let discovered = state
            .channels
            .rs485
            .discovered
            .iter()
            .map(i8::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        text(self.params.rs485_discovered, &discovered);
        if let Some(target) = state.channels.rs485.selected_target {
            set(self.params.rs485_target, Value::I32(i32::from(target)));
        }
        let stats = &state.channels.rs485.diagnostics;
        set(self.params.rs485_stats.tx, Value::I64(stats.tx as i64));
        set(self.params.rs485_stats.rx, Value::I64(stats.rx as i64));
        set(self.params.rs485_stats.acks, Value::I64(stats.acks as i64));
        set(
            self.params.rs485_stats.ack_timeouts,
            Value::I64(stats.ack_timeouts as i64),
        );
        set(
            self.params.rs485_stats.decode_errors,
            Value::I64(stats.decode_errors as i64),
        );
        set(
            self.params.rs485_stats.outbox,
            Value::I64(stats.outbox as i64),
        );

        set(self.params.dut_present, Value::Bool(state.dut.present));
        set(
            self.params.dut_firmware_kind,
            Value::Enum(firmware_value(state.dut.firmware)),
        );
        text(
            self.params.dut_version,
            state.dut.version.as_deref().unwrap_or(""),
        );
        set(
            self.params.dut_uptime_s,
            Value::I32(state.dut.uptime_s.unwrap_or(0)),
        );
        set(
            self.params.dut_ratio,
            Value::Enum(ratio_value(state.dut.ratio)),
        );
        set(
            self.params.dut_usteps_per_rev,
            Value::I32(state.dut.usteps_per_rev.unwrap_or(0)),
        );

        publish_axis(&self.bus, &self.params.axis_a, state.dut.axis(Axis::A));
        publish_axis(&self.bus, &self.params.axis_b, state.dut.axis(Axis::B));

        match state.dut.threshold {
            Some(threshold) => {
                set(
                    self.params.threshold_floor,
                    Value::I32(threshold.floor as i32),
                );
                set(
                    self.params.threshold_band,
                    Value::I32(threshold.band as i32),
                );
                set(
                    self.params.threshold_applied,
                    Value::I32(threshold.applied as i32),
                );
                set(
                    self.params.threshold_calibrated_at_s,
                    Value::I32(threshold.calibrated_at_s),
                );
            }
            // -1 is "never this session", and the page draws that as its own alarming state
            // rather than as a row of zeroes.
            None => set(self.params.threshold_calibrated_at_s, Value::I32(-1)),
        }

        set(self.params.faults_active, Value::I32(state.faults as i32));
        set(
            self.params.counts_passed,
            Value::I32(self.bench.passed as i32),
        );
        set(
            self.params.counts_failed,
            Value::I32(self.bench.failed as i32),
        );
        set(
            self.params.counts_aborted,
            Value::I32(self.bench.aborted as i32),
        );

        let status = self.bench.run_status();
        set(self.params.run_busy, Value::Bool(status.is_some()));
        match &status {
            Some(status) => {
                text(self.params.run_plan, &status.plan);
                set(
                    self.params.run_phase,
                    Value::Enum(phase_value(status.phase)),
                );
                set(
                    self.params.run_origin,
                    Value::Enum(origin_value(status.origin)),
                );
                text(self.params.run_step_name, &status.step_name);
                set(
                    self.params.run_step_index,
                    Value::I32(status.step_index as i32),
                );
                set(
                    self.params.run_step_count,
                    Value::I32(status.step_count as i32),
                );
                set(
                    self.params.run_step_fraction,
                    Value::F64(match status.step_count {
                        0 => 0.0,
                        count => status.step_index as f64 / count as f64,
                    }),
                );
                set(self.params.run_cycle, Value::I32(status.cycle as i32));
                set(
                    self.params.run_cycle_count,
                    Value::I32(status.cycle_count as i32),
                );
                set(self.params.run_elapsed_s, Value::I32(status.elapsed_s));
                // -1 is "unknown", and it stays that way until there is a real basis for an
                // estimate. A made-up ETA on a routine whose duration varies with how cold the
                // module is would be worse than none: people plan around it.
                set(self.params.run_eta_s, Value::I32(-1));
            }
            None => {
                set(self.params.run_phase, Value::Enum(phase_value(Phase::Idle)));
                set(self.params.run_origin, Value::Enum(0));
                text(self.params.run_step_name, "");
                set(self.params.run_step_index, Value::I32(0));
                set(self.params.run_step_count, Value::I32(0));
                set(self.params.run_step_fraction, Value::F64(0.0));
            }
        }
        *self.shared.run.lock().unwrap() = status;

        *self.shared.state.lock().unwrap() = state;
        self.publish_flash();
        self.publish_log(now);
        self.publish_telemetry();
    }

    fn publish_flash(&self) {
        let snapshot = self.flash.snapshot();
        let p = &self.params;
        let _ = self.bus.set(p.flash.armed, Value::Bool(snapshot.armed));
        let _ = self.bus.set(p.flash.busy, Value::Bool(snapshot.busy));
        let _ = self.bus.set_text(p.flash.phase, &snapshot.phase);
        let _ = self.bus.set_text(p.flash.step, &snapshot.step);
        let _ = self
            .bus
            .set(p.flash.progress, Value::F64(snapshot.progress));
        let _ = self.bus.set_text(p.flash.detail, &snapshot.detail);
        let _ = self.bus.set_text(p.flash.boot_state, &snapshot.boot_state);
        let _ = self
            .bus
            .set_text(p.flash.boot_detail, &snapshot.boot_detail);
        let _ = self
            .bus
            .set(p.flash.needs_replug, Value::Bool(snapshot.needs_replug));
        let _ = self
            .bus
            .set_text(p.flash.last_outcome, &snapshot.last_outcome);
        let _ = self.bus.set_text(p.flash.scope, &snapshot.scope);

        let _ = self
            .bus
            .set(p.probe.connected, Value::Bool(snapshot.probe_connected));
        let _ = self
            .bus
            .set(p.probe.target_present, Value::Bool(snapshot.target_present));
        let _ = self.bus.set_text(p.probe.name, &snapshot.probe_name);
        let _ = self.bus.set_text(p.probe.serial, &snapshot.probe_serial);
        let _ = self
            .bus
            .set_text(p.probe.firmware, &snapshot.probe_firmware);
        let _ = self
            .bus
            .set(p.probe.speed_khz, Value::I32(snapshot.speed_khz as i32));

        if let Some(mcu) = &snapshot.mcu {
            let _ = self.bus.set_text(p.mcu.part, &mcu.part);
            let _ = self.bus.set_text(p.mcu.uid, &mcu.uid);
            let _ = self.bus.set_text(p.mcu.idcode, &mcu.idcode);
            let _ = self.bus.set_text(p.mcu.dev_id, &mcu.dev_id);
            let _ = self.bus.set_text(p.mcu.layout, &mcu.layout);
            let _ = self.bus.set_text(p.mcu.rdp, &mcu.rdp);
            let _ = self.bus.set_text(p.mcu.firmware, &mcu.firmware);
            let _ = self
                .bus
                .set(p.mcu.flash_kb, Value::I32(i32::from(mcu.flash_kb)));
        }
        *self.shared.flash.lock().unwrap() = snapshot.clone();
    }

    fn publish_log(&mut self, _now: u64) {
        let lines: Vec<serde_json::Value> = self
            .bench
            .log()
            .since(self.log_cursor)
            .iter()
            .map(|line| {
                serde_json::json!({
                    "seq": line.seq,
                    "at_ms": line.at_ms,
                    "level": line.level,
                    "source": line.source,
                    "message": line.message,
                })
            })
            .collect();
        if lines.is_empty() {
            return;
        }
        self.log_cursor = self.bench.log().next_seq();
        let mut shared = self.shared.log.lock().unwrap();
        shared.extend(lines);
        // The bench's own ring is the scrollback; this mirror only needs to be bounded so a
        // long soak does not grow it without limit.
        let excess = shared.len().saturating_sub(bench_core::state::LOG_CAPACITY);
        if excess > 0 {
            shared.drain(..excess);
        }
    }

    fn publish_telemetry(&mut self) {
        let samples: Vec<serde_json::Value> = self
            .bench
            .telemetry()
            .since(self.telemetry_cursor)
            .iter()
            .map(|sample| serde_json::to_value(sample).expect("telemetry sample serialises"))
            .collect();
        if samples.is_empty() {
            return;
        }
        self.telemetry_cursor = self.bench.telemetry().next_seq();
        let mut shared = self.shared.telemetry.lock().unwrap();
        shared.extend(samples);
        let excess = shared
            .len()
            .saturating_sub(bench_core::state::TELEMETRY_CAPACITY);
        if excess > 0 {
            shared.drain(..excess);
        }
    }

    fn publish_outcome(&mut self, outcome: &Outcome) {
        let set = |id, value| {
            let _ = self.bus.set(id, value);
        };
        let text = |id, value: &str| {
            let _ = self.bus.set_text(id, value);
        };

        set(
            self.params.last_verdict,
            Value::Enum(verdict_value(outcome.verdict.name())),
        );
        text(self.params.last_plan, &outcome.plan);
        text(self.params.last_reason, &outcome.verdict.reason());
        text(
            self.params.last_advice,
            match &outcome.verdict {
                bench_core::verdict::Verdict::Error { advice, .. } => {
                    advice.as_deref().unwrap_or("")
                }
                _ => "",
            },
        );
        text(self.params.last_measurements, &outcome.summary());
        text(
            self.params.last_report_path,
            outcome.report_path.as_deref().unwrap_or(""),
        );
        self.last_seq += 1;
        set(self.params.last_seq, Value::I64(self.last_seq));

        self.cue(match outcome.verdict {
            bench_core::verdict::Verdict::Pass => "pass",
            bench_core::verdict::Verdict::Fail { .. } => "fail",
            _ => "abort",
        });
    }
}

fn publish_link(
    bus: &Bus,
    params: &schema::LinkParams,
    state: &bench_core::state::ChannelState,
    value: fn(Option<LinkKind>) -> u32,
) {
    let _ = bus.set(params.connected, Value::Bool(state.link.connected));
    let _ = bus.set(params.observed, Value::Enum(value(state.link.kind)));
    let _ = bus.set_text(params.detail, state.link.detail.as_deref().unwrap_or(""));
}

fn serial_transport_value(kind: Option<LinkKind>) -> u32 {
    match kind {
        Some(LinkKind::Vcp) => 1,
        Some(LinkKind::BenchAscii) => 2,
        _ => 0,
    }
}

fn rs485_transport_value(kind: Option<LinkKind>) -> u32 {
    match kind {
        Some(LinkKind::Rs485Serial) => 1,
        Some(LinkKind::Rs485Tcp) => 2,
        _ => 0,
    }
}

fn publish_axis(bus: &Bus, params: &AxisParams, state: &bench_core::state::AxisState) {
    let set = |id, value| {
        let _ = bus.set(id, value);
    };
    set(params.position, Value::I32(state.position.unwrap_or(0)));
    set(params.target, Value::I32(state.target.unwrap_or(0)));
    // A flag nobody has reported stays false on the bus; the page decides how to draw that
    // using `/dut/present`, so "not measured" and "measured and bad" stay distinguishable.
    set(params.position_known, Value::Bool(state.position.is_some()));
    set(params.health_known, Value::Bool(state.health.is_some()));
    let health = state.health.unwrap_or_default();
    set(params.health_home, Value::Bool(health.home_ok));
    set(params.health_switches, Value::Bool(health.switches_ok));
    set(params.health_backlash, Value::Bool(health.backlash_ok));
    set(
        params.health_measure_cycle,
        Value::Bool(health.measure_cycle_ok),
    );
}

// --- enum mapping, by name in both directions ------------------------------------------

pub fn transport_from(value: u32) -> Option<LinkKind> {
    match schema::TRANSPORTS
        .iter()
        .find(|(v, _)| *v == value)
        .map(|(_, n)| *n)
    {
        Some("vcp") => Some(LinkKind::Vcp),
        Some("bench-ascii") => Some(LinkKind::BenchAscii),
        Some("rs485-serial") => Some(LinkKind::Rs485Serial),
        Some("rs485-tcp") => Some(LinkKind::Rs485Tcp),
        Some("sim") => Some(LinkKind::Sim),
        _ => None,
    }
}

fn by_name(table: &[(u32, &str)], name: &str) -> u32 {
    table
        .iter()
        .find(|(_, n)| *n == name)
        .map(|(v, _)| *v)
        .unwrap_or(0)
}

fn transport_value(kind: LinkKind) -> u32 {
    by_name(schema::TRANSPORTS, kind.name())
}

fn firmware_value(kind: FirmwareKind) -> u32 {
    by_name(
        schema::FIRMWARE_KINDS,
        match kind {
            FirmwareKind::Unknown => "unknown",
            FirmwareKind::Production => "production",
            FirmwareKind::Bench => "bench",
            FirmwareKind::BootloaderOnly => "bootloader-only",
        },
    )
}

fn ratio_value(ratio: GearRatio) -> u32 {
    by_name(
        schema::GEAR_RATIOS,
        match ratio {
            GearRatio::Unknown => "unknown",
            GearRatio::R16 => "16",
            GearRatio::R32 => "32",
        },
    )
}

fn phase_value(phase: Phase) -> u32 {
    by_name(schema::RUN_PHASES, phase.name())
}

fn origin_value(origin: Origin) -> u32 {
    by_name(schema::RUN_ORIGINS, origin.name())
}

fn verdict_value(name: &str) -> u32 {
    by_name(schema::VERDICTS, name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every enum this worker publishes must survive the name round trip. Keying a page on a
    /// discriminant is the failure these tables exist to prevent, and a name that does not
    /// appear in its table silently becomes 0 — which is `none`, `unknown` or `idle`.
    #[test]
    fn every_enum_maps_to_a_declared_variant_by_name() {
        for kind in [
            LinkKind::Vcp,
            LinkKind::BenchAscii,
            LinkKind::Rs485Serial,
            LinkKind::Rs485Tcp,
            LinkKind::Sim,
        ] {
            let value = transport_value(kind);
            assert_eq!(
                transport_from(value),
                Some(kind),
                "{kind:?} did not round trip"
            );
        }

        // A non-default variant must not collapse to 0.
        assert_ne!(firmware_value(FirmwareKind::Production), 0);
        assert_ne!(ratio_value(GearRatio::R32), 0);
        assert_ne!(phase_value(Phase::Body), 0);
        assert_ne!(origin_value(Origin::Agent), 0);
        assert_ne!(verdict_value("pass"), 0);
    }

    #[test]
    fn the_declared_action_list_and_the_resolved_one_are_the_same() {
        // `declare` and `resolve` both iterate `ACTIONS`, so this guards the list itself
        // against duplicates, which would make one action shadow another.
        let mut sorted = schema::ACTIONS.to_vec();
        sorted.sort_unstable();
        let count = sorted.len();
        sorted.dedup();
        assert_eq!(sorted.len(), count, "duplicate action names");
    }
}
