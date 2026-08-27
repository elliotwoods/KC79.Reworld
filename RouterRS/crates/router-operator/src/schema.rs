//! The parameter tree, the telemetry channels, and the ids the bridge writes through.
//!
//! Follows the PortalTestBench idioms:
//! - **actions are monotonic `i64` counters** — the bridge acts on the *change*, so a page
//!   that reconnects mid-session re-triggers nothing;
//! - **desired and observed are separate** — writable params carry operator intent, `.read_only()`
//!   mirrors carry what the model actually holds, and drawing the disagreement is the point;
//! - **scalars live here; documents travel over `/api/router/*`** (portal firmware logs, the
//!   diagnostics tables, serial-port and file listings, the renderer source documents).
//!
//! # Dynamic structure
//!
//! The schema is sealed, but the installation is not: columns are rebuilt, sources are added
//! and removed. Boundary events re-seal the whole schema (`LiveBus::reseal`) — never at frame
//! rate. Per-column (`/columns/N/*`) and per-source (`/sources/N/*`) subtrees are declared per
//! instance; per-portal editable state sits behind ONE selection-proxy subtree (`/portal/*`)
//! driven by `/ui/select/*`, exactly matching the iced app's single-selection inspector. Live
//! data for *all* portals rides telemetry, indexed by slot (see [`slot_offsets`]).

use av_gui_bus::{Bus, ParamId, SampleType, SchemaBuilder, TelemetryId, Unit, Value};
use router_core::config::AppConfig;

// ------------------------------------------------------------------ enum tables
//
// Read back **by name** on the page, never by discriminant. The `Individual` display spelling
// is deliberate: the config *file* keeps the C++ typo "Inidividual" (config.rs), the operator
// never sees it.

pub const TRANSMIT_MODES: &[(u32, &str)] = &[(0, "Individual"), (1, "Keyframe"), (2, "Disabled")];
pub const LEADING_CONTROLS: &[(u32, &str)] = &[(0, "Position"), (1, "Polar"), (2, "Axes")];
pub const SELECT_KINDS: &[(u32, &str)] = &[
    (0, "installation"),
    (1, "column"),
    (2, "portal"),
    (3, "source"),
];
pub const PORTAL_SUBS: &[(u32, &str)] = &[
    (0, "overview"),
    (1, "pilot"),
    (2, "axis_a"),
    (3, "axis_b"),
    (4, "motor"),
    (5, "log"),
];
/// Composite styles — the serialized strings ("HV_ThetaR") stay backend-only; these are the
/// operator-facing names in source order (`image/sources/mod.rs::Style`).
pub const SOURCE_STYLES: &[(u32, &str)] = &[(0, "Direct"), (1, "HV Theta-R"), (2, "Centered")];
pub const GRADIENT_TYPES: &[(u32, &str)] = &[(0, "Radial"), (1, "Horizontal"), (2, "Vertical")];
pub const GRADIENT_WAVES: &[(u32, &str)] = &[(0, "Triangle"), (1, "Sine"), (2, "Sawtooth")];
pub const LOOP_MODES: &[(u32, &str)] = &[(0, "Loop"), (1, "Ping Pong"), (2, "None")];

/// The broadcastable hardware actions, in `ActionKind::ALL` order, preceded by Poll. The name
/// is the schema leaf; the bridge maps it back to `ActionKind` (or `Command::Poll`).
pub const BROADCAST_ACTIONS: &[&str] = &[
    "poll",
    "ping",
    "init",
    "calibrate",
    "home",
    "flash_leds",
    "go_home",
    "see_through",
    "lights_off",
    "lights_on",
    "unjam",
    "escape",
    "reboot",
];

/// Local pilot actions on the selected portal (no broadcast; `Command` variants).
pub const PORTAL_LOCAL_ACTIONS: &[&str] = &[
    "reset_local",
    "unwind",
    "push",
    "poll_position",
    "take_current",
    "see_through_local",
];

/// Per-axis motion-control routines (`McCommand`) plus the motor-driver tests.
pub const AXIS_ACTIONS: &[&str] = &[
    "zero_position",
    "measure_backlash",
    "home_routine",
    "init_timer",
    "deinit_timer",
    "test_timer",
    "push_profile",
    "md_test_routine",
    "md_test_timer",
];

// ------------------------------------------------------------------ declaration

type DeclResult = Result<(), String>;

fn check(result: Result<ParamId, av_gui_bus::BusError>) -> DeclResult {
    result.map(|_| ()).map_err(|error| error.to_string())
}

fn action(builder: &mut SchemaBuilder, path: &str, label: &str) -> DeclResult {
    check(builder.param(path).i64(0).label(label).register())
}

fn action_bank(builder: &mut SchemaBuilder, prefix: &str, names: &[&str]) -> DeclResult {
    for name in names {
        action(
            builder,
            &format!("{prefix}/actions/{name}"),
            &title_case(name),
        )?;
    }
    Ok(())
}

fn title_case(name: &str) -> String {
    let mut out = String::new();
    for (i, word) in name.split('_').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let mut chars = word.chars();
        if i == 0 {
            if let Some(first) = chars.next() {
                out.extend(first.to_uppercase());
            }
        } else if let Some(first) = chars.next() {
            out.push(first);
        }
        out.extend(chars);
    }
    out
}

/// The per-column portal capacity, in declaration order. Portal slot for telemetry =
/// `offset[col] + (target_id - 1)`; published as JSON in `/installation/slot_offsets` so the
/// page's lane math survives heterogeneous column shapes.
pub fn slot_offsets(shapes: &[(usize, usize)]) -> Vec<usize> {
    let mut offsets = Vec::with_capacity(shapes.len());
    let mut total = 0usize;
    for (count_x, count_y) in shapes {
        offsets.push(total);
        total += count_x * count_y;
    }
    offsets
}

/// The shape the schema was declared for. The bridge compares the live snapshot against this
/// to decide when a re-seal is due, and the telemetry writers size their rows from it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Shape {
    /// Per column: (count_x, count_y).
    pub columns: Vec<(usize, usize)>,
    /// Preview image resolution (w, h).
    pub resolution: (usize, usize),
    /// Renderer source type names, in order.
    pub sources: Vec<String>,
}

impl Shape {
    pub fn total_portals(&self) -> usize {
        self.columns.iter().map(|(x, y)| x * y).sum()
    }

    pub fn from_config(config: &AppConfig) -> Self {
        let arrangement = &config.installation.arrangement;
        let columns = (0..arrangement.columns)
            .map(|i| {
                let over = config.installation.columns.get(i);
                (
                    over.map_or(arrangement.column_width, |c| c.count_x),
                    over.map_or(arrangement.rows, |c| c.count_y),
                )
            })
            .collect::<Vec<_>>();
        let resolution = match columns.first() {
            Some((x, _)) => (columns.len() * x, arrangement.rows),
            None => (0, 0),
        };
        let sources = config
            .renderer_sources
            .iter()
            .filter_map(|s| s.get("type").and_then(|t| t.as_str()).map(String::from))
            .collect();
        Self {
            columns,
            resolution,
            sources,
        }
    }
}

/// Declare the whole tree for one shape. Called at startup from the loaded config, and again
/// (through the bridge) with a fresh builder whenever the live shape changes.
pub fn declare(builder: &mut SchemaBuilder, shape: &Shape, _simulated: bool) -> DeclResult {
    // --- the page's own liveness ----------------------------------------------------------
    check(
        builder
            .param("/ui/heartbeat")
            .i64(0)
            .label("UI heartbeat")
            .register(),
    )?;

    // --- selection (drives the /portal proxy; bus state so proxy and page agree) ----------
    check(
        builder
            .param("/ui/select/kind")
            .enumeration(0, SELECT_KINDS)
            .label("Selection kind")
            .register(),
    )?;
    check(
        builder
            .param("/ui/select/col")
            .i32(0)
            .label("Selected column")
            .register(),
    )?;
    check(
        builder
            .param("/ui/select/portal")
            .i32(1)
            .label("Selected portal")
            .register(),
    )?;
    check(
        builder
            .param("/ui/select/sub")
            .enumeration(0, PORTAL_SUBS)
            .label("Portal panel")
            .register(),
    )?;
    check(
        builder
            .param("/ui/select/source")
            .i32(0)
            .label("Selected source")
            .register(),
    )?;

    // --- setup facts ----------------------------------------------------------------------
    check(
        builder
            .param("/app/simulated")
            .bool(false)
            .label("Simulated")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/app/http_port")
            .i32(0)
            .label("HTTP port")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/app/config_path")
            .text("")
            .label("Config file")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/app/version")
            .text("")
            .label("Version")
            .read_only()
            .register(),
    )?;

    // --- installation ---------------------------------------------------------------------
    check(
        builder
            .param("/installation/arrangement/columns")
            .i32(0)
            .range(1.0, 64.0)
            .label("Columns")
            .register(),
    )?;
    check(
        builder
            .param("/installation/arrangement/rows")
            .i32(0)
            .range(1.0, 64.0)
            .label("Rows")
            .register(),
    )?;
    check(
        builder
            .param("/installation/arrangement/column_width")
            .i32(0)
            .range(1.0, 16.0)
            .label("Column width")
            .register(),
    )?;
    check(
        builder
            .param("/installation/arrangement/panel_height")
            .i32(0)
            .range(0.0, 16.0)
            .label("Panel height")
            .register(),
    )?;
    check(
        builder
            .param("/installation/arrangement/flipped")
            .bool(false)
            .label("Flipped")
            .register(),
    )?;
    check(
        builder
            .param("/installation/messaging/transmit")
            .enumeration(0, TRANSMIT_MODES)
            .label("Transmit")
            .register(),
    )?;
    check(
        builder
            .param("/installation/messaging/period_s")
            .f32(0.5)
            .range(0.0, 10.0)
            .step(0.01)
            .unit(Unit::Seconds)
            .label("Period")
            .register(),
    )?;
    check(
        builder
            .param("/installation/messaging/keyframe_batch")
            .i32(8)
            .range(1.0, 64.0)
            .label("Keyframe batch size")
            .register(),
    )?;
    check(
        builder
            .param("/installation/messaging/keyframe_velocities")
            .bool(true)
            .label("Keyframe velocities")
            .register(),
    )?;
    check(
        builder
            .param("/installation/image_enabled")
            .bool(false)
            .label("Image sampling")
            .register(),
    )?;
    // The "Pilot all" drag pad: one Vec2, clamped to the unit circle on the page; the bridge
    // broadcasts a collateable {"m":[a,b]} through the first portal's kinematics on change.
    check(
        builder
            .param("/installation/pilot_all")
            .vec2([0.0, 0.0])
            .label("Pilot all")
            .no_reset()
            .register(),
    )?;
    action_bank(builder, "/installation", BROADCAST_ACTIONS)?;
    action(
        builder,
        "/installation/actions/home_and_zero",
        "Home and zero local",
    )?;
    action(
        builder,
        "/installation/actions/rebuild_columns",
        "Rebuild columns",
    )?;
    action(builder, "/installation/actions/save_config", "Save config")?;
    check(
        builder
            .param("/installation/resolution")
            .vec2([shape.resolution.0 as f32, shape.resolution.1 as f32])
            .label("Image resolution")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/installation/slot_offsets")
            .text(&serde_json::to_string(&slot_offsets(&shape.columns)).unwrap_or_default())
            .label("Portal slot offsets")
            .read_only()
            .register(),
    )?;

    // --- bulk hardware knobs (whole-installation commands) --------------------------------
    check(
        builder
            .param("/bulk/max_velocity")
            .i32(30000)
            .range(1.0, 200000.0)
            .label("Max velocity")
            .register(),
    )?;
    check(
        builder
            .param("/bulk/acceleration")
            .i32(10000)
            .range(1.0, 200000.0)
            .label("Acceleration")
            .register(),
    )?;
    check(
        builder
            .param("/bulk/current_amps")
            .f32(0.25)
            .range(0.0, 0.3)
            .step(0.005)
            .unit(Unit::Amps)
            .label("Current")
            .register(),
    )?;
    action(
        builder,
        "/bulk/actions/push_motion_profile",
        "Push motion profile to all",
    )?;
    action(builder, "/bulk/actions/set_current", "Set current on all")?;

    // --- servers (observed; runtime enable/port is config-file territory today) -----------
    check(
        builder
            .param("/servers/osc/running")
            .bool(false)
            .label("OSC running")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/servers/osc/port")
            .i32(0)
            .label("OSC port")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/servers/rest/running")
            .bool(false)
            .label("REST running")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/servers/rest/port")
            .i32(0)
            .label("REST port")
            .read_only()
            .register(),
    )?;

    // --- session / report ------------------------------------------------------------------
    check(
        builder
            .param("/report/session_file")
            .text("")
            .label("Session file")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/report/file_bytes")
            .i64(0)
            .label("Session size")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/report/dropped_events")
            .i64(0)
            .label("Dropped events")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/report/verbose")
            .bool(false)
            .label("Verbose packet log")
            .register(),
    )?;
    check(
        builder
            .param("/report/marker_text")
            .text("")
            .label("Marker")
            .register(),
    )?;
    action(builder, "/report/actions/mark", "Write marker")?;
    action(
        builder,
        "/report/actions/write_summary",
        "Write summary now",
    )?;
    check(
        builder
            .param("/stats/tx_per_s")
            .f32(0.0)
            .label("Tx rate")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/stats/rx_per_s")
            .f32(0.0)
            .label("Rx rate")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/health/faulty_units")
            .i32(0)
            .label("Faulty units")
            .read_only()
            .register(),
    )?;

    // --- per-column subtrees ---------------------------------------------------------------
    for (i, (count_x, count_y)) in shape.columns.iter().enumerate() {
        let p = |leaf: &str| format!("/columns/{i}/{leaf}");
        check(
            builder
                .param(&p("shape"))
                .vec2([*count_x as f32, *count_y as f32])
                .label("Portals (x, y)")
                .read_only()
                .register(),
        )?;
        check(
            builder
                .param(&p("flipped"))
                .bool(false)
                .label("Flipped")
                .read_only()
                .register(),
        )?;
        check(
            builder
                .param(&p("rs485/connected"))
                .bool(false)
                .label("Connected")
                .read_only()
                .register(),
        )?;
        check(
            builder
                .param(&p("rs485/device_description"))
                .text("")
                .label("Device")
                .read_only()
                .register(),
        )?;
        // Desired device, as the same JSON the config carries:
        // {"deviceType":"Serial"|"TCP","address":...,"port"?}. Written by the device picker;
        // applied by the connect action.
        check(
            builder
                .param(&p("rs485/device"))
                .text("")
                .label("Device settings")
                .register(),
        )?;
        check(
            builder
                .param(&p("rs485/tx_count"))
                .i64(0)
                .label("Tx")
                .read_only()
                .register(),
        )?;
        check(
            builder
                .param(&p("rs485/rx_count"))
                .i64(0)
                .label("Rx")
                .read_only()
                .register(),
        )?;
        check(
            builder
                .param(&p("rs485/ack_timeouts"))
                .i64(0)
                .label("ACK timeouts")
                .read_only()
                .register(),
        )?;
        check(
            builder
                .param(&p("rs485/decode_errors"))
                .i64(0)
                .label("Decode errors")
                .read_only()
                .register(),
        )?;
        check(
            builder
                .param(&p("scheduled_poll/enabled"))
                .bool(false)
                .label("Scheduled poll")
                .register(),
        )?;
        check(
            builder
                .param(&p("scheduled_poll/period_s"))
                .f32(60.0)
                .range(0.01, 100.0)
                .unit(Unit::Seconds)
                .label("Poll period")
                .register(),
        )?;
        check(
            builder
                .param(&p("pilot_all"))
                .vec2([0.0, 0.0])
                .label("Pilot all")
                .no_reset()
                .register(),
        )?;
        action_bank(builder, &format!("/columns/{i}"), BROADCAST_ACTIONS)?;
        action(builder, &p("actions/connect"), "Connect")?;
        action(builder, &p("actions/disconnect"), "Disconnect")?;
        action(builder, &p("actions/clear_outbox"), "Clear outbox")?;
        action(builder, &p("actions/clear_counters"), "Clear counters")?;
    }

    // --- the portal selection proxy (static paths; repointed by /ui/select) ----------------
    check(
        builder
            .param("/portal/exists")
            .bool(false)
            .label("Portal found")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/portal/target_id")
            .i32(0)
            .label("Target ID")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/portal/pilot/leading")
            .enumeration(2, LEADING_CONTROLS)
            .label("Leading control")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/portal/pilot/position")
            .vec2([0.0, 0.0])
            .range(-1.0, 1.0)
            .step(0.001)
            .label("Position")
            .no_reset()
            .register(),
    )?;
    check(
        builder
            .param("/portal/pilot/polar")
            .vec2([0.0, 0.0])
            .range(-std::f64::consts::PI, std::f64::consts::PI)
            .step(0.001)
            .label("Polar (r, θ)")
            .no_reset()
            .register(),
    )?;
    check(
        builder
            .param("/portal/pilot/axes")
            .vec2([0.0, 0.0])
            .range(0.0, 1.0)
            .step(0.0001)
            .label("Axes (a, b)")
            .no_reset()
            .register(),
    )?;
    check(
        builder
            .param("/portal/pilot/offset")
            .f32(0.0)
            .range(-0.25, 0.25)
            .step(0.001)
            .label("Offset")
            .register(),
    )?;
    check(
        builder
            .param("/portal/pilot/send_periodically")
            .bool(false)
            .label("Send periodically")
            .register(),
    )?;
    check(
        builder
            .param("/portal/poll/regularly")
            .bool(false)
            .label("Poll regularly")
            .register(),
    )?;
    check(
        builder
            .param("/portal/poll/interval_s")
            .f32(1.0)
            .range(0.01, 60.0)
            .unit(Unit::Seconds)
            .label("Poll interval")
            .register(),
    )?;
    check(
        builder
            .param("/portal/mds/current_amps")
            .f32(0.25)
            .range(0.0, 0.3)
            .step(0.005)
            .unit(Unit::Amps)
            .label("Current")
            .register(),
    )?;
    check(
        builder
            .param("/portal/mds/microstep_resolution")
            .enumeration(5, MICROSTEP_RESOLUTIONS)
            .label("Microstep resolution")
            .register(),
    )?;
    for axis in ["a", "b"] {
        let p = |leaf: &str| format!("/portal/axis/{axis}/{leaf}");
        check(
            builder
                .param(&p("profile/max_velocity"))
                .i32(30000)
                .range(1.0, 200000.0)
                .label("Max velocity")
                .register(),
        )?;
        check(
            builder
                .param(&p("profile/acceleration"))
                .i32(10000)
                .range(1.0, 200000.0)
                .label("Acceleration")
                .register(),
        )?;
        check(
            builder
                .param(&p("profile/min_velocity"))
                .i32(100)
                .range(1.0, 100000.0)
                .label("Min velocity")
                .register(),
        )?;
        check(
            builder
                .param(&p("reported_position"))
                .i32(0)
                .label("Position")
                .read_only()
                .register(),
        )?;
        check(
            builder
                .param(&p("reported_target"))
                .i32(0)
                .label("Target")
                .read_only()
                .register(),
        )?;
        // −1 unknown / 0 fault / 1 ok
        check(
            builder
                .param(&p("health_ok"))
                .i32(-1)
                .label("Health")
                .read_only()
                .register(),
        )?;
        action_bank(builder, &format!("/portal/axis/{axis}"), &[])?;
        for name in AXIS_ACTIONS {
            action(builder, &p(&format!("actions/{name}")), &title_case(name))?;
        }
    }
    check(
        builder
            .param("/portal/state/uptime_ms")
            .i64(0)
            .label("Uptime")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/portal/state/version")
            .text("")
            .label("Firmware")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/portal/state/in_position")
            .bool(false)
            .label("In position")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/portal/state/last_log")
            .text("")
            .label("Last log")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/portal/state/last_log_level")
            .i32(0)
            .label("Last log level")
            .read_only()
            .register(),
    )?;
    action_bank(builder, "/portal", BROADCAST_ACTIONS)?;
    for name in PORTAL_LOCAL_ACTIONS {
        action(
            builder,
            &format!("/portal/actions/{name}"),
            &title_case(name),
        )?;
    }
    action(builder, "/portal/log/actions/clear", "Clear log")?;

    // --- renderer sources (per instance; re-sealed on add/remove) --------------------------
    for (i, type_name) in shape.sources.iter().enumerate() {
        declare_source(builder, i, type_name)?;
    }
    action(builder, "/sources/actions/add_gradient", "Add gradient")?;
    action(builder, "/sources/actions/add_text", "Add text")?;
    action(
        builder,
        "/sources/actions/add_file_player",
        "Add file player",
    )?;
    action(builder, "/sources/actions/add_spout", "Add Spout")?;

    // --- telemetry -------------------------------------------------------------------------
    let total = shape.total_portals().max(1) as u32;
    let tel =
        |r: Result<TelemetryId, av_gui_bus::BusError>| r.map(|_| ()).map_err(|e| e.to_string());
    // Per portal ×4: target x, y, live x, y (NaN = unreported).
    tel(builder.telemetry("/tel/portals/pose", SampleType::F32, total * 4, 8, 30.0))?;
    // Per portal ×4: rx age ms, tx age ms, health state (0..4), health score (0..100).
    tel(builder.telemetry("/tel/portals/link", SampleType::F32, total * 4, 8, 10.0))?;
    // Per column ×4: rx age ms, tx age ms, outbox size, connected.
    tel(builder.telemetry(
        "/tel/columns/link",
        SampleType::F32,
        (shape.columns.len().max(1) as u32) * 4,
        16,
        30.0,
    ))?;
    // The selected portal, at model rate; the ring is the history the sparkline draws.
    // Lanes: pos x, y, polar r, θ, axes a, b, live axes a, b, live pos x, y,
    //        live tgt x, y, mc pos a, mc tgt a, mc pos b, mc tgt b, in-position, rx age ms.
    tel(builder.telemetry("/tel/portal/selected", SampleType::F32, 18, 1024, 62.5))?;
    // OSC messages seen per model tick.
    tel(builder.telemetry("/tel/osc", SampleType::F32, 1, 1024, 30.0))?;
    // The composited preview, RGB bytes.
    let (w, h) = shape.resolution;
    tel(builder.telemetry(
        "/tel/preview",
        SampleType::U8,
        ((w * h * 3).max(3)) as u32,
        4,
        15.0,
    ))?;

    Ok(())
}

/// Valid TMC microstep resolutions; transmitted to hardware as log2 (config.rs / portal.rs).
pub const MICROSTEP_RESOLUTIONS: &[(u32, &str)] = &[
    (0, "1"),
    (1, "2"),
    (2, "4"),
    (3, "8"),
    (4, "16"),
    (5, "32"),
    (6, "64"),
    (7, "128"),
    (8, "256"),
];

fn declare_source(builder: &mut SchemaBuilder, i: usize, type_name: &str) -> DeclResult {
    let p = |leaf: &str| format!("/sources/{i}/{leaf}");
    check(
        builder
            .param(&p("type"))
            .text(type_name)
            .label("Type")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param(&p("visible"))
            .bool(true)
            .label("Visible")
            .register(),
    )?;
    check(
        builder
            .param(&p("render_enabled"))
            .bool(true)
            .label("Render")
            .register(),
    )?;
    check(
        builder
            .param(&p("alpha"))
            .f32(1.0)
            .range(0.0, 1.0)
            .step(0.01)
            .label("Alpha")
            .register(),
    )?;
    check(
        builder
            .param(&p("style"))
            .enumeration(0, SOURCE_STYLES)
            .label("Composite style")
            .register(),
    )?;
    match type_name {
        "Gradient" => {
            check(
                builder
                    .param(&p("gradient_type"))
                    .enumeration(0, GRADIENT_TYPES)
                    .label("Gradient type")
                    .register(),
            )?;
            check(
                builder
                    .param(&p("wave"))
                    .enumeration(0, GRADIENT_WAVES)
                    .label("Wave")
                    .register(),
            )?;
            check(
                builder
                    .param(&p("frequency"))
                    .f32(1.0)
                    .range(0.0, 8.0)
                    .step(0.05)
                    .label("Frequency")
                    .register(),
            )?;
            check(
                builder
                    .param(&p("speed"))
                    .f32(0.05)
                    .range(-2.0, 2.0)
                    .step(0.01)
                    .label("Speed")
                    .register(),
            )?;
            check(
                builder
                    .param(&p("value1"))
                    .vec2([0.0, 0.0])
                    .range(-1.0, 1.0)
                    .step(0.01)
                    .label("Value 1")
                    .register(),
            )?;
            check(
                builder
                    .param(&p("value2"))
                    .vec2([1.0, 1.0])
                    .range(-1.0, 1.0)
                    .step(0.01)
                    .label("Value 2")
                    .register(),
            )?;
        }
        "Text" => {
            check(
                builder
                    .param(&p("text"))
                    .text("TEST")
                    .label("Text")
                    .register(),
            )?;
            check(builder.param(&p("font")).text("").label("Font").register())?;
            check(
                builder
                    .param(&p("size"))
                    .i32(11)
                    .range(1.0, 200.0)
                    .label("Size")
                    .register(),
            )?;
            check(
                builder
                    .param(&p("border"))
                    .i32(0)
                    .range(0.0, 8.0)
                    .label("Border")
                    .register(),
            )?;
            check(
                builder
                    .param(&p("inverse"))
                    .bool(false)
                    .label("Inverse")
                    .register(),
            )?;
        }
        "FilePlayer" => {
            check(
                builder
                    .param(&p("file"))
                    .text("")
                    .label("File")
                    .read_only()
                    .register(),
            )?;
            check(
                builder
                    .param(&p("play"))
                    .bool(true)
                    .label("Play")
                    .register(),
            )?;
            check(
                builder
                    .param(&p("loop_mode"))
                    .enumeration(0, LOOP_MODES)
                    .label("Loop mode")
                    .register(),
            )?;
            check(
                builder
                    .param(&p("speed"))
                    .f32(1.0)
                    .range(-4.0, 4.0)
                    .step(0.05)
                    .label("Speed")
                    .register(),
            )?;
            check(
                builder
                    .param(&p("position"))
                    .f32(0.0)
                    .range(0.0, 1.0)
                    .step(0.001)
                    .label("Position")
                    .no_reset()
                    .register(),
            )?;
            check(
                builder
                    .param(&p("loaded"))
                    .bool(false)
                    .label("Loaded")
                    .read_only()
                    .register(),
            )?;
            check(
                builder
                    .param(&p("duration_s"))
                    .f32(0.0)
                    .unit(Unit::Seconds)
                    .label("Duration")
                    .read_only()
                    .register(),
            )?;
            check(
                builder
                    .param(&p("error"))
                    .text("")
                    .label("Error")
                    .read_only()
                    .register(),
            )?;
            action(builder, &p("actions/clear_file"), "Clear file")?;
            action(builder, &p("actions/jump_to_start"), "Jump to start")?;
        }
        // Spout is Windows-only; the page shows the card only when these paths exist.
        "Spout" => {
            check(
                builder
                    .param(&p("sender_name"))
                    .text("")
                    .label("Sender")
                    .register(),
            )?;
            check(
                builder
                    .param(&p("status"))
                    .text("")
                    .label("Status")
                    .read_only()
                    .register(),
            )?;
        }
        _ => {}
    }
    action(builder, &p("actions/remove"), "Remove source")?;
    Ok(())
}

// ------------------------------------------------------------------ resolved ids

/// Ids the bridge writes through every tick, resolved once per (re)seal. Only params touched
/// at tick rate are resolved eagerly; action counters and desired params are discovered by
/// path scan in the bridge (they scale with the shape).
pub struct Params {
    pub app_simulated: ParamId,
    pub app_http_port: ParamId,
    pub app_config_path: ParamId,
    pub app_version: ParamId,

    pub arrangement_columns: ParamId,
    pub arrangement_rows: ParamId,
    pub arrangement_column_width: ParamId,
    pub arrangement_panel_height: ParamId,
    pub arrangement_flipped: ParamId,
    pub messaging_transmit: ParamId,
    pub messaging_period_s: ParamId,
    pub messaging_keyframe_batch: ParamId,
    pub messaging_keyframe_velocities: ParamId,
    pub image_enabled: ParamId,
    pub installation_pilot_all: ParamId,
    pub resolution: ParamId,
    pub slot_offsets: ParamId,

    pub bulk_max_velocity: ParamId,
    pub bulk_acceleration: ParamId,
    pub bulk_current_amps: ParamId,

    pub select_kind: ParamId,
    pub select_col: ParamId,
    pub select_portal: ParamId,

    pub servers_osc_running: ParamId,
    pub servers_osc_port: ParamId,
    pub servers_rest_running: ParamId,
    pub servers_rest_port: ParamId,

    pub report_session_file: ParamId,
    pub report_file_bytes: ParamId,
    pub report_dropped: ParamId,
    pub report_verbose: ParamId,
    pub report_marker_text: ParamId,
    pub stats_tx_per_s: ParamId,
    pub stats_rx_per_s: ParamId,
    pub health_faulty_units: ParamId,

    pub columns: Vec<ColumnParams>,
    pub portal: PortalParams,
    pub sources: Vec<SourceParams>,

    pub tel_pose: TelemetryId,
    pub tel_link: TelemetryId,
    pub tel_columns: TelemetryId,
    pub tel_selected: TelemetryId,
    pub tel_osc: TelemetryId,
    pub tel_preview: TelemetryId,
}

pub struct ColumnParams {
    pub connected: ParamId,
    pub device_description: ParamId,
    pub device: ParamId,
    pub tx_count: ParamId,
    pub rx_count: ParamId,
    pub ack_timeouts: ParamId,
    pub decode_errors: ParamId,
    pub scheduled_poll_enabled: ParamId,
    pub scheduled_poll_period_s: ParamId,
    pub pilot_all: ParamId,
}

pub struct AxisParams {
    pub max_velocity: ParamId,
    pub acceleration: ParamId,
    pub min_velocity: ParamId,
    pub reported_position: ParamId,
    pub reported_target: ParamId,
    pub health_ok: ParamId,
}

pub struct PortalParams {
    pub exists: ParamId,
    pub target_id: ParamId,
    pub leading: ParamId,
    pub position: ParamId,
    pub polar: ParamId,
    pub axes: ParamId,
    pub offset: ParamId,
    pub send_periodically: ParamId,
    pub poll_regularly: ParamId,
    pub poll_interval_s: ParamId,
    pub mds_current_amps: ParamId,
    pub mds_microstep_resolution: ParamId,
    pub axis: [AxisParams; 2],
    pub uptime_ms: ParamId,
    pub version: ParamId,
    pub in_position: ParamId,
    pub last_log: ParamId,
    pub last_log_level: ParamId,
}

pub struct SourceParams {
    pub visible: ParamId,
    pub render_enabled: ParamId,
    pub alpha: ParamId,
    pub style: ParamId,
    /// (leaf name, id) for the type-specific writable params.
    pub extras: Vec<(String, ParamId)>,
    /// Read-only mirrors kept fresh from the snapshot (FilePlayer loaded/duration/error, file).
    pub mirrors: Vec<(String, ParamId)>,
}

impl Params {
    pub fn resolve(bus: &Bus, shape: &Shape) -> Result<Self, String> {
        let id = |path: &str| -> Result<ParamId, String> {
            bus.id_of(path)
                .ok_or_else(|| format!("schema path missing: {path}"))
        };
        let tel = |path: &str| -> Result<TelemetryId, String> {
            bus.schema()
                .telemetry_id_of(path)
                .ok_or_else(|| format!("telemetry path missing: {path}"))
        };
        let columns = (0..shape.columns.len())
            .map(|i| -> Result<ColumnParams, String> {
                let p = |leaf: &str| id(&format!("/columns/{i}/{leaf}"));
                Ok(ColumnParams {
                    connected: p("rs485/connected")?,
                    device_description: p("rs485/device_description")?,
                    device: p("rs485/device")?,
                    tx_count: p("rs485/tx_count")?,
                    rx_count: p("rs485/rx_count")?,
                    ack_timeouts: p("rs485/ack_timeouts")?,
                    decode_errors: p("rs485/decode_errors")?,
                    scheduled_poll_enabled: p("scheduled_poll/enabled")?,
                    scheduled_poll_period_s: p("scheduled_poll/period_s")?,
                    pilot_all: p("pilot_all")?,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let axis = |name: &str| -> Result<AxisParams, String> {
            let p = |leaf: &str| id(&format!("/portal/axis/{name}/{leaf}"));
            Ok(AxisParams {
                max_velocity: p("profile/max_velocity")?,
                acceleration: p("profile/acceleration")?,
                min_velocity: p("profile/min_velocity")?,
                reported_position: p("reported_position")?,
                reported_target: p("reported_target")?,
                health_ok: p("health_ok")?,
            })
        };
        let sources = (0..shape.sources.len())
            .map(|i| -> Result<SourceParams, String> {
                let base = format!("/sources/{i}");
                let mut extras = Vec::new();
                let mut mirrors = Vec::new();
                for decl in bus.schema().params() {
                    let path = decl.path.as_str();
                    let Some(leaf) = path.strip_prefix(&format!("{base}/")) else {
                        continue;
                    };
                    if leaf.starts_with("actions/")
                        || matches!(
                            leaf,
                            "type" | "visible" | "render_enabled" | "alpha" | "style"
                        )
                    {
                        continue;
                    }
                    let pid = id(path)?;
                    if decl.is_read_only() {
                        mirrors.push((leaf.to_string(), pid));
                    } else {
                        extras.push((leaf.to_string(), pid));
                    }
                }
                Ok(SourceParams {
                    visible: id(&format!("{base}/visible"))?,
                    render_enabled: id(&format!("{base}/render_enabled"))?,
                    alpha: id(&format!("{base}/alpha"))?,
                    style: id(&format!("{base}/style"))?,
                    extras,
                    mirrors,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            app_simulated: id("/app/simulated")?,
            app_http_port: id("/app/http_port")?,
            app_config_path: id("/app/config_path")?,
            app_version: id("/app/version")?,
            arrangement_columns: id("/installation/arrangement/columns")?,
            arrangement_rows: id("/installation/arrangement/rows")?,
            arrangement_column_width: id("/installation/arrangement/column_width")?,
            arrangement_panel_height: id("/installation/arrangement/panel_height")?,
            arrangement_flipped: id("/installation/arrangement/flipped")?,
            messaging_transmit: id("/installation/messaging/transmit")?,
            messaging_period_s: id("/installation/messaging/period_s")?,
            messaging_keyframe_batch: id("/installation/messaging/keyframe_batch")?,
            messaging_keyframe_velocities: id("/installation/messaging/keyframe_velocities")?,
            image_enabled: id("/installation/image_enabled")?,
            installation_pilot_all: id("/installation/pilot_all")?,
            resolution: id("/installation/resolution")?,
            slot_offsets: id("/installation/slot_offsets")?,
            bulk_max_velocity: id("/bulk/max_velocity")?,
            bulk_acceleration: id("/bulk/acceleration")?,
            bulk_current_amps: id("/bulk/current_amps")?,
            select_kind: id("/ui/select/kind")?,
            select_col: id("/ui/select/col")?,
            select_portal: id("/ui/select/portal")?,
            servers_osc_running: id("/servers/osc/running")?,
            servers_osc_port: id("/servers/osc/port")?,
            servers_rest_running: id("/servers/rest/running")?,
            servers_rest_port: id("/servers/rest/port")?,
            report_session_file: id("/report/session_file")?,
            report_file_bytes: id("/report/file_bytes")?,
            report_dropped: id("/report/dropped_events")?,
            report_verbose: id("/report/verbose")?,
            report_marker_text: id("/report/marker_text")?,
            stats_tx_per_s: id("/stats/tx_per_s")?,
            stats_rx_per_s: id("/stats/rx_per_s")?,
            health_faulty_units: id("/health/faulty_units")?,
            columns,
            portal: PortalParams {
                exists: id("/portal/exists")?,
                target_id: id("/portal/target_id")?,
                leading: id("/portal/pilot/leading")?,
                position: id("/portal/pilot/position")?,
                polar: id("/portal/pilot/polar")?,
                axes: id("/portal/pilot/axes")?,
                offset: id("/portal/pilot/offset")?,
                send_periodically: id("/portal/pilot/send_periodically")?,
                poll_regularly: id("/portal/poll/regularly")?,
                poll_interval_s: id("/portal/poll/interval_s")?,
                mds_current_amps: id("/portal/mds/current_amps")?,
                mds_microstep_resolution: id("/portal/mds/microstep_resolution")?,
                axis: [axis("a")?, axis("b")?],
                uptime_ms: id("/portal/state/uptime_ms")?,
                version: id("/portal/state/version")?,
                in_position: id("/portal/state/in_position")?,
                last_log: id("/portal/state/last_log")?,
                last_log_level: id("/portal/state/last_log_level")?,
            },
            sources,
            tel_pose: tel("/tel/portals/pose")?,
            tel_link: tel("/tel/portals/link")?,
            tel_columns: tel("/tel/columns/link")?,
            tel_selected: tel("/tel/portal/selected")?,
            tel_osc: tel("/tel/osc")?,
            tel_preview: tel("/tel/preview")?,
        })
    }
}

/// Publish the startup facts the page shows in its title/status chrome.
pub fn publish_setup(
    bus: &Bus,
    params: &Params,
    port: u16,
    simulated: bool,
    config_path: &std::path::Path,
) -> Result<(), String> {
    let set = |id: ParamId, value: Value| bus.set(id, value).map_err(|e| e.to_string());
    set(params.app_simulated, Value::Bool(simulated))?;
    set(params.app_http_port, Value::I32(i32::from(port)))?;
    bus.set_text(params.app_config_path, &config_path.display().to_string())
        .map_err(|e| e.to_string())?;
    bus.set_text(params.app_version, env!("CARGO_PKG_VERSION"))
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape() -> Shape {
        Shape {
            columns: vec![(3, 6); 4],
            resolution: (12, 6),
            sources: vec!["Gradient".into(), "Text".into(), "FilePlayer".into()],
        }
    }

    #[test]
    fn declares_and_resolves() {
        let mut builder = SchemaBuilder::new();
        declare(&mut builder, &shape(), true).expect("declare");
        let bus = builder.seal();
        let params = Params::resolve(&bus, &shape()).expect("resolve");
        assert_eq!(params.columns.len(), 4);
        assert_eq!(params.sources.len(), 3);
        // Gradient extras include value1/value2; FilePlayer mirrors include loaded.
        assert!(
            params.sources[0]
                .extras
                .iter()
                .any(|(leaf, _)| leaf == "value1")
        );
        assert!(
            params.sources[2]
                .mirrors
                .iter()
                .any(|(leaf, _)| leaf == "loaded")
        );
    }

    #[test]
    fn slot_offsets_accumulate() {
        assert_eq!(slot_offsets(&[(3, 6), (3, 6), (2, 4)]), vec![0, 18, 36]);
    }

    #[test]
    fn shape_from_config_matches_installation() {
        let config = AppConfig::from_json(serde_json::json!({
            "Installation": {
                "arrangement": { "Columns": 2, "Rows": 3, "Column width": 1 },
            }
        }));
        let shape = Shape::from_config(&config);
        assert_eq!(shape.columns, vec![(1, 3), (1, 3)]);
        assert_eq!(shape.resolution, (2, 3));
    }
}
