//! The parameter tree, and the ids the worker writes through.
//!
//! # Desired and observed are separate
//!
//! `/transport/desired` is what the operator asked for; `/transport/observed` is what the
//! bench actually holds. They disagree while a link is opening, while it is retrying, and
//! permanently when the port is gone -- and drawing that disagreement is the point. The same
//! split applies to motion: a target position and a real one are two marks, never one.
//!
//! # Actions are counters
//!
//! `/actions/*` bump an `i64` rather than toggling a flag. The worker acts on the *change*, so
//! pressing "Home A" twice homes twice, and a page that reconnects mid-run does not re-trigger
//! anything it happens to find set. "Try that again" is a request; "connected" is a state.
//!
//! # What is a parameter and what is not
//!
//! Scalars an operator reads or sets live here. A run's measurement table, the merged log ring
//! and the list of discovered artefacts do not: those are documents, and they travel over
//! `/api/bench/*` where an agent can also read them with `curl`. The bus is the human's
//! channel; HTTP is the agent's. Both feed one command queue, so they cannot diverge.

use av_gui_bus::{Bus, ParamId, SchemaBuilder, Value};

/// Enum variants, declared once and read back **by name** on the page. Never by discriminant:
/// a page keyed on `2` inverts silently the moment someone reorders one of these lists.
pub const TRANSPORTS: &[(u32, &str)] = &[
    (0, "none"),
    // Direct USB serial to the module's own USART1. The production firmware's text menu.
    (1, "vcp"),
    // The bench firmware's ASCII line protocol, same physical port.
    (2, "bench-ascii"),
    // RS485 through a router board on a COM port.
    (3, "rs485-serial"),
    // RS485 through an Ethernet gateway, raw TCP.
    (4, "rs485-tcp"),
    (5, "sim"),
];

pub const SERIAL_DIALECTS: &[(u32, &str)] = &[(0, "none"), (1, "vcp"), (2, "bench-ascii")];
pub const RS485_TRANSPORTS: &[(u32, &str)] = &[(0, "none"), (1, "rs485-serial"), (2, "rs485-tcp")];
pub const CHANNELS: &[(u32, &str)] = &[(0, "serial"), (1, "rs485")];
pub const LINE_ENDINGS: &[(u32, &str)] = &[(0, "none"), (1, "cr"), (2, "lf"), (3, "crlf")];

/// Which firmware image is on the module. Not a preference -- it decides which commands exist.
pub const FIRMWARE_KINDS: &[(u32, &str)] = &[
    (0, "unknown"),
    (1, "production"),
    (2, "bench"),
    (3, "bootloader-only"),
];

/// The prism gearing. Auto-detected by the firmware's home routine; never assumed.
pub const GEAR_RATIOS: &[(u32, &str)] = &[(0, "unknown"), (1, "16"), (2, "32")];

/// Where a run has got to. `escaping` is its own phase because it is the one an operator most
/// needs to see: an abort is not instant, and pretending it is hides the motion still happening.
pub const RUN_PHASES: &[(u32, &str)] = &[
    (0, "idle"),
    (1, "preflight"),
    (2, "setup"),
    (3, "body"),
    (4, "teardown"),
    (5, "escaping"),
    (6, "done"),
];

/// Who started the run in flight. The human at the GUI must never be surprised about who is
/// driving the hardware in front of them.
pub const RUN_ORIGINS: &[(u32, &str)] = &[(0, "none"), (1, "gui"), (2, "agent"), (3, "cli")];

pub const VERDICTS: &[(u32, &str)] = &[
    (0, "none"),
    (1, "pass"),
    (2, "fail"),
    (3, "aborted"),
    (4, "error"),
];

/// One-shot events the page turns into sound. Paired with a monotonic `/cue_seq` so a
/// reconnecting page adopts the current sequence rather than replaying the last cue.
pub const CUES: &[(u32, &str)] = &[
    (0, "none"),
    (1, "connected"),
    (2, "lost"),
    (3, "run-start"),
    (4, "pass"),
    (5, "fail"),
    (6, "abort"),
    (7, "attention"),
];

/// Declare the whole tree.
///
/// `simulated` gates the `/sim/*` controls: the production schema never carries dead controls,
/// and the page detects simulation by looking for those declarations rather than being told.
pub fn declare(builder: &mut SchemaBuilder, simulated: bool) -> Result<(), String> {
    let check = |result: Result<ParamId, av_gui_bus::BusError>| -> Result<(), String> {
        result.map(|_| ()).map_err(|error| error.to_string())
    };

    // --- the page's own liveness -----------------------------------------------------------
    //
    // Unlike the flasher's, this heartbeat does NOT cancel work when it goes stale. It blocks
    // *starting* something destructive, and nothing else. Closing a browser tab must never
    // abort an eight-hour soak; that would be a worse failure than a silent one. See AGENTS.md.
    check(
        builder
            .param("/ui/heartbeat")
            .i64(0)
            .label("UI heartbeat")
            .register(),
    )?;

    // --- transport ---------------------------------------------------------------------------
    check(
        builder
            .param("/transport/desired")
            .enumeration(0, TRANSPORTS)
            .label("Transport")
            .register(),
    )?;
    check(
        builder
            .param("/transport/observed")
            .enumeration(0, TRANSPORTS)
            .label("Transport (actual)")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/transport/port")
            .text("")
            .label("Port")
            .register(),
    )?;
    check(
        builder
            .param("/transport/connected")
            .bool(false)
            .label("Connected")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/transport/detail")
            .text("")
            .label("Link detail")
            .read_only()
            .register(),
    )?;

    // Independent lanes. `/transport/*` remains as a compatibility mirror of the active lane.
    check(
        builder
            .param("/serial/desired")
            .enumeration(0, SERIAL_DIALECTS)
            .label("Serial protocol")
            .register(),
    )?;
    check(
        builder
            .param("/serial/observed")
            .enumeration(0, SERIAL_DIALECTS)
            .label("Serial protocol (actual)")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/serial/port")
            .text("")
            .label("Serial port")
            .register(),
    )?;
    check(
        builder
            .param("/serial/connected")
            .bool(false)
            .label("Serial connected")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/serial/detail")
            .text("")
            .label("Serial detail")
            .read_only()
            .register(),
    )?;

    check(
        builder
            .param("/rs485/desired")
            .enumeration(0, RS485_TRANSPORTS)
            .label("RS485 transport")
            .register(),
    )?;
    check(
        builder
            .param("/rs485/observed")
            .enumeration(0, RS485_TRANSPORTS)
            .label("RS485 transport (actual)")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/rs485/endpoint")
            .text("")
            .label("RS485 endpoint")
            .register(),
    )?;
    check(
        builder
            .param("/rs485/connected")
            .bool(false)
            .label("RS485 connected")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/rs485/detail")
            .text("")
            .label("RS485 detail")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/rs485/target")
            .i32(1)
            .range(1.0, 127.0)
            .label("Target address")
            .register(),
    )?;
    check(
        builder
            .param("/rs485/discovered")
            .text("")
            .label("Discovered addresses")
            .read_only()
            .register(),
    )?;
    for (leaf, label) in [
        ("tx", "TX packets"),
        ("rx", "RX packets"),
        ("acks", "Acknowledgements"),
        ("ack_timeouts", "ACK timeouts"),
        ("decode_errors", "Decode errors"),
        ("outbox", "Queued packets"),
    ] {
        check(
            builder
                .param(&format!("/rs485/stats/{leaf}"))
                .i64(0)
                .label(label)
                .read_only()
                .register(),
        )?;
    }

    // The route used by direct Operations and pilot controls. Plans keep their own validation.
    check(
        builder
            .param("/motion/route")
            .enumeration(0, CHANNELS)
            .label("Command via")
            .register(),
    )?;
    for axis in ["a", "b"] {
        check(
            builder
                .param(&format!("/motion/{axis}/rotations"))
                .f64(0.0)
                .range(-1000.0, 1000.0)
                .label(&format!("{} target rotations", axis.to_uppercase()))
                .register(),
        )?;
    }
    check(
        builder
            .param("/motion/profile/max_velocity")
            .i32(24_000)
            .range(1.0, 100_000.0)
            .label("Maximum velocity")
            .register(),
    )?;
    check(
        builder
            .param("/motion/profile/acceleration")
            .i32(20_000)
            .range(1.0, 250_000.0)
            .label("Acceleration")
            .register(),
    )?;
    check(
        builder
            .param("/motion/profile/min_velocity")
            .i32(1_000)
            .range(0.0, 100_000.0)
            .label("Minimum velocity")
            .register(),
    )?;

    // Advanced testing controls. They remain schema parameters so the human page and an agent
    // use the same worker-owned queue and can never race the physical links.
    check(
        builder
            .param("/test/raw/vcom_text")
            .text("")
            .label("VCOM payload")
            .register(),
    )?;
    check(
        builder
            .param("/test/raw/line_ending")
            .enumeration(0, LINE_ENDINGS)
            .label("Line ending")
            .register(),
    )?;
    check(
        builder
            .param("/test/raw/rs485_json")
            .text("{\"poll\":null}")
            .label("RS485 MessagePack body")
            .register(),
    )?;
    check(
        builder
            .param("/test/current_a")
            .f64(0.15)
            .range(0.05, 1.0)
            .label("Motor current")
            .register(),
    )?;
    check(
        builder
            .param("/test/microstep")
            .i32(16)
            .range(1.0, 256.0)
            .label("Microstep resolution")
            .register(),
    )?;
    check(
        builder
            .param("/test/home_threshold")
            .i32(246)
            .range(0.0, 255.0)
            .label("Home threshold")
            .register(),
    )?;
    check(
        builder
            .param("/test/census_speed")
            .i32(0)
            .range(0.0, 100_000.0)
            .label("Census speed")
            .register(),
    )?;

    // SWD flasher. High-volume artefact/device documents stay on HTTP; these are the live facts
    // and choices an operator needs beside the irreversible controls.
    check(
        builder
            .param("/flash/auto_enabled")
            .bool(false)
            .label("Auto-flash")
            .register(),
    )?;
    check(
        builder
            .param("/flash/armed")
            .bool(false)
            .label("Auto-flash armed")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/flash/busy")
            .bool(false)
            .label("Flashing")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/flash/phase")
            .text("disarmed")
            .label("Flash phase")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/flash/step")
            .text("")
            .label("Flash step")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/flash/progress")
            .f64(0.0)
            .range(0.0, 1.0)
            .label("Flash progress")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/flash/detail")
            .text("")
            .label("Flash detail")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/flash/boot_state")
            .text("unknown")
            .label("Post-flash boot")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/flash/boot_detail")
            .text("")
            .label("Post-flash boot detail")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/flash/needs_replug")
            .bool(false)
            .label("Power cycle required")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/flash/last_outcome")
            .text("")
            .label("Last flash result")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/flash/boot_id")
            .text("")
            .label("Bootloader image")
            .register(),
    )?;
    check(
        builder
            .param("/flash/app_id")
            .text("")
            .label("Application image")
            .register(),
    )?;
    check(
        builder
            .param("/flash/scope")
            .text("nothing")
            .label("Flash scope")
            .read_only()
            .register(),
    )?;

    check(
        builder
            .param("/probe/selected")
            .text("")
            .label("Selected ST-Link")
            .register(),
    )?;
    check(
        builder
            .param("/probe/connected")
            .bool(false)
            .label("ST-Link connected")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/probe/target_present")
            .bool(false)
            .label("MCU present")
            .read_only()
            .register(),
    )?;
    for (leaf, label) in [
        ("name", "ST-Link"),
        ("serial", "ST-Link serial"),
        ("firmware", "ST-Link firmware"),
    ] {
        check(
            builder
                .param(&format!("/probe/{leaf}"))
                .text("")
                .label(label)
                .read_only()
                .register(),
        )?;
    }
    check(
        builder
            .param("/probe/speed_khz")
            .i32(0)
            .label("SWD speed [kHz]")
            .read_only()
            .register(),
    )?;

    for (leaf, label) in [
        ("part", "MCU"),
        ("uid", "MCU UID"),
        ("idcode", "DBGMCU IDCODE"),
        ("dev_id", "MCU DEV_ID"),
        ("layout", "Flash layout"),
        ("rdp", "RDP level"),
        ("firmware", "Firmware in flash"),
    ] {
        check(
            builder
                .param(&format!("/mcu/{leaf}"))
                .text("")
                .label(label)
                .read_only()
                .register(),
        )?;
    }
    check(
        builder
            .param("/mcu/flash_kb")
            .i32(0)
            .label("MCU flash [kB]")
            .read_only()
            .register(),
    )?;

    // --- the module under test ---------------------------------------------------------------
    check(
        builder
            .param("/dut/present")
            .bool(false)
            .label("Module present")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/dut/firmware_kind")
            .enumeration(0, FIRMWARE_KINDS)
            .label("Firmware")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/dut/version")
            .text("")
            .label("Version")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/dut/uptime_s")
            .i32(0)
            .label("Uptime")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/dut/ratio")
            .enumeration(0, GEAR_RATIOS)
            .label("Gearing")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/dut/usteps_per_rev")
            .i32(0)
            .label("Microsteps / rev")
            .read_only()
            .register(),
    )?;

    for axis in ["a", "b"] {
        let up = axis.to_uppercase();
        check(
            builder
                .param(&format!("/dut/{axis}/position"))
                .i32(0)
                .label(&format!("{up} position"))
                .read_only()
                .register(),
        )?;
        check(
            builder
                .param(&format!("/dut/{axis}/target"))
                .i32(0)
                .label(&format!("{up} target"))
                .read_only()
                .register(),
        )?;
        // Whether a position has EVER been reported for this axis. `/dut/present` means a
        // module answered; it does not mean it reports positions. The VCP menu never does, so
        // without this the page draws a confident 0 for an axis nothing has measured.
        check(
            builder
                .param(&format!("/dut/{axis}/position_known"))
                .bool(false)
                .label(&format!("{up} position known"))
                .read_only()
                .register(),
        )?;
        check(
            builder
                .param(&format!("/dut/{axis}/health_known"))
                .bool(false)
                .label(&format!("{up} health known"))
                .read_only()
                .register(),
        )?;
        for (flag, label) in [
            ("home", "homed"),
            ("switches", "switches"),
            ("backlash", "backlash"),
            ("measure_cycle", "measure cycle"),
        ] {
            check(
                builder
                    .param(&format!("/dut/{axis}/health_{flag}"))
                    .bool(false)
                    .label(&format!("{up} {label}"))
                    .read_only()
                    .register(),
            )?;
        }
    }

    // --- the optical threshold ---------------------------------------------------------------
    //
    // On screen at all times, never behind a dialog. This is the single most-misused number in
    // the rig: the background it used to be derived from is unmeasurable on the production
    // ring, so the operating point comes from a per-run census of the flag itself. Hiding these
    // three numbers is exactly how a fixed constant creeps back in.
    check(
        builder
            .param("/dut/threshold/floor")
            .i32(0)
            .label("Threshold floor")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/dut/threshold/band")
            .i32(0)
            .label("Threshold band")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/dut/threshold/applied")
            .i32(0)
            .label("Threshold applied")
            .read_only()
            .register(),
    )?;
    // -1 means "never calibrated this session", which is a distinct and alarming state, not a
    // zero. Anything that homes optically without this being set is refused before it starts.
    check(
        builder
            .param("/dut/threshold/calibrated_at_s")
            .i32(-1)
            .label("Calibrated")
            .read_only()
            .register(),
    )?;

    // --- what to run -------------------------------------------------------------------------
    //
    // Writable, unlike `/run/plan`, which reports the plan actually in flight. Two parameters
    // rather than one because "what I intend to run next" and "what is running" disagree for
    // the whole length of a run, and that disagreement is exactly what an operator needs to see.
    check(
        builder
            .param("/plan/selected")
            .text("startup")
            .label("Plan")
            .register(),
    )?;

    // --- the run in flight ---------------------------------------------------------------------
    check(
        builder
            .param("/run/busy")
            .bool(false)
            .label("Running")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/run/plan")
            .text("")
            .label("Plan")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/run/phase")
            .enumeration(0, RUN_PHASES)
            .label("Phase")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/run/origin")
            .enumeration(0, RUN_ORIGINS)
            .label("Started by")
            .read_only()
            .register(),
    )?;
    // Names the firmware routine actually in flight, so the ACK-early-then-run gap reads as
    // "Home A" rather than a spinner. Routines take seconds to minutes.
    check(
        builder
            .param("/run/step_name")
            .text("")
            .label("Step")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/run/step_index")
            .i32(0)
            .label("Step index")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/run/step_count")
            .i32(0)
            .label("Steps")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/run/step_fraction")
            .f64(0.0)
            .range(0.0, 1.0)
            .label("Step progress")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/run/cycle")
            .i32(0)
            .label("Cycle")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/run/cycle_count")
            .i32(0)
            .label("Cycles")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/run/elapsed_s")
            .i32(0)
            .label("Elapsed")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/run/eta_s")
            .i32(-1)
            .label("ETA")
            .read_only()
            .register(),
    )?;

    // --- the last answer -----------------------------------------------------------------------
    check(
        builder
            .param("/last/verdict")
            .enumeration(0, VERDICTS)
            .label("Verdict")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/last/plan")
            .text("")
            .label("Plan")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/last/reason")
            .text("")
            .label("Reason")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/last/advice")
            .text("")
            .label("What to do")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/last/measurements")
            .text("")
            .label("Measurements")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/last/report_path")
            .text("")
            .label("Report")
            .read_only()
            .register(),
    )?;
    // Monotonic: distinguishes a fresh result from a repaint of the same one.
    check(
        builder
            .param("/last/seq")
            .i64(0)
            .label("Result sequence")
            .read_only()
            .register(),
    )?;

    check(
        builder
            .param("/counts/passed")
            .i32(0)
            .label("Passed")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/counts/failed")
            .i32(0)
            .label("Failed")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/counts/aborted")
            .i32(0)
            .label("Aborted")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/faults/active")
            .i32(0)
            .label("Active faults")
            .read_only()
            .register(),
    )?;

    check(
        builder
            .param("/cue")
            .enumeration(0, CUES)
            .label("Cue")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/cue_seq")
            .i64(0)
            .label("Cue sequence")
            .read_only()
            .register(),
    )?;

    // --- how this bench is configured ------------------------------------------------------------
    //
    // All read-only, all on screen behind a ParamTree. A measurement whose conditions are not
    // recorded is not evidence, and the first question about a surprising result is always
    // "what was it set to".
    check(
        builder
            .param("/setup/http_port")
            .i32(0)
            .label("HTTP port")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/setup/simulated")
            .bool(false)
            .label("Simulated")
            .read_only()
            .register(),
    )?;
    check(
        builder
            .param("/setup/report_profile")
            .text("")
            .label("Report profile")
            .read_only()
            .register(),
    )?;
    // 300 ms is the C++ Router's `responseWindow_ms`, preserved bit-for-bit by RouterRS and
    // therefore by this bench: a timing change would make bus measurements incomparable.
    check(
        builder
            .param("/setup/response_window_ms")
            .i32(300)
            .label("ACK window")
            .read_only()
            .register(),
    )?;
    // Above this, a move is refused outside a `characterise` plan. The 32:1 stall cliff sits at
    // 30-32k microsteps/s when ramped at 100k/s^2.
    check(
        builder
            .param("/setup/stall_guard_usteps_per_s")
            .i32(28_000)
            .label("Stall guard")
            .read_only()
            .register(),
    )?;
    // 100-250 mA is indistinguishable in slip and in homing precision, so there are no dynamic
    // current games -- one fixed value plus the driver's own auto-sleep.
    check(
        builder
            .param("/setup/default_current_a")
            .f64(0.15)
            .label("Default current")
            .read_only()
            .register(),
    )?;

    // --- actions -----------------------------------------------------------------------------------
    for name in ACTIONS {
        check(
            builder
                .param(&format!("/actions/{name}"))
                .i64(0)
                .label(&action_label(name))
                .register(),
        )?;
    }

    if simulated {
        check(
            builder
                .param("/sim/module_present")
                .bool(true)
                .label("Module present")
                .register(),
        )?;
        check(
            builder
                .param("/sim/fail_next_run")
                .bool(false)
                .label("Fail the next run")
                .register(),
        )?;
    }

    Ok(())
}

/// The ids of one axis's parameters.
pub struct AxisParams {
    pub position: ParamId,
    pub target: ParamId,
    pub health_home: ParamId,
    pub health_switches: ParamId,
    pub health_backlash: ParamId,
    pub health_measure_cycle: ParamId,
    pub position_known: ParamId,
    pub health_known: ParamId,
}

pub struct LinkParams {
    pub desired: ParamId,
    pub observed: ParamId,
    pub endpoint: ParamId,
    pub connected: ParamId,
    pub detail: ParamId,
}

pub struct Rs485StatsParams {
    pub tx: ParamId,
    pub rx: ParamId,
    pub acks: ParamId,
    pub ack_timeouts: ParamId,
    pub decode_errors: ParamId,
    pub outbox: ParamId,
}

pub struct MotionParams {
    pub route: ParamId,
    pub a_rotations: ParamId,
    pub b_rotations: ParamId,
    pub max_velocity: ParamId,
    pub acceleration: ParamId,
    pub min_velocity: ParamId,
}

pub struct TestParams {
    pub raw_vcom_text: ParamId,
    pub raw_line_ending: ParamId,
    pub raw_rs485_json: ParamId,
    pub current_a: ParamId,
    pub microstep: ParamId,
    pub home_threshold: ParamId,
    pub census_speed: ParamId,
}

pub struct FlashParams {
    pub auto_enabled: ParamId,
    pub armed: ParamId,
    pub busy: ParamId,
    pub phase: ParamId,
    pub step: ParamId,
    pub progress: ParamId,
    pub detail: ParamId,
    pub boot_state: ParamId,
    pub boot_detail: ParamId,
    pub needs_replug: ParamId,
    pub last_outcome: ParamId,
    pub boot_id: ParamId,
    pub app_id: ParamId,
    pub scope: ParamId,
}

pub struct ProbeParams {
    pub selected: ParamId,
    pub connected: ParamId,
    pub target_present: ParamId,
    pub name: ParamId,
    pub serial: ParamId,
    pub firmware: ParamId,
    pub speed_khz: ParamId,
}

pub struct McuParams {
    pub part: ParamId,
    pub uid: ParamId,
    pub idcode: ParamId,
    pub dev_id: ParamId,
    pub layout: ParamId,
    pub rdp: ParamId,
    pub firmware: ParamId,
    pub flash_kb: ParamId,
}

/// Every id the worker writes through, resolved once at start so no tick does a string lookup.
pub struct Params {
    pub ui_heartbeat: ParamId,

    pub transport_desired: ParamId,
    pub transport_observed: ParamId,
    pub transport_port: ParamId,
    pub transport_connected: ParamId,
    pub transport_detail: ParamId,

    pub serial: LinkParams,
    pub rs485: LinkParams,
    pub rs485_target: ParamId,
    pub rs485_discovered: ParamId,
    pub rs485_stats: Rs485StatsParams,
    pub motion: MotionParams,
    pub test: TestParams,
    pub flash: FlashParams,
    pub probe: ProbeParams,
    pub mcu: McuParams,

    pub dut_present: ParamId,
    pub dut_firmware_kind: ParamId,
    pub dut_version: ParamId,
    pub dut_uptime_s: ParamId,
    pub dut_ratio: ParamId,
    pub dut_usteps_per_rev: ParamId,
    pub axis_a: AxisParams,
    pub axis_b: AxisParams,

    pub threshold_floor: ParamId,
    pub threshold_band: ParamId,
    pub threshold_applied: ParamId,
    pub threshold_calibrated_at_s: ParamId,

    pub plan_selected: ParamId,
    pub run_busy: ParamId,
    pub run_plan: ParamId,
    pub run_phase: ParamId,
    pub run_origin: ParamId,
    pub run_step_name: ParamId,
    pub run_step_index: ParamId,
    pub run_step_count: ParamId,
    pub run_step_fraction: ParamId,
    pub run_cycle: ParamId,
    pub run_cycle_count: ParamId,
    pub run_elapsed_s: ParamId,
    pub run_eta_s: ParamId,

    pub last_verdict: ParamId,
    pub last_plan: ParamId,
    pub last_reason: ParamId,
    pub last_advice: ParamId,
    pub last_measurements: ParamId,
    pub last_report_path: ParamId,
    pub last_seq: ParamId,

    pub counts_passed: ParamId,
    pub counts_failed: ParamId,
    pub counts_aborted: ParamId,
    pub faults_active: ParamId,

    pub cue: ParamId,
    pub cue_seq: ParamId,

    pub setup_http_port: ParamId,
    pub setup_simulated: ParamId,
    pub setup_report_profile: ParamId,

    /// Action counters, in the order the worker checks them.
    pub actions: Vec<(&'static str, ParamId)>,
}

impl Params {
    pub fn resolve(bus: &Bus) -> Result<Self, String> {
        let id = |path: &str| {
            bus.id_of(path)
                .ok_or_else(|| format!("{path} is not in the sealed schema"))
        };
        let axis = |name: &str| -> Result<AxisParams, String> {
            Ok(AxisParams {
                position: id(&format!("/dut/{name}/position"))?,
                target: id(&format!("/dut/{name}/target"))?,
                health_home: id(&format!("/dut/{name}/health_home"))?,
                health_switches: id(&format!("/dut/{name}/health_switches"))?,
                health_backlash: id(&format!("/dut/{name}/health_backlash"))?,
                health_measure_cycle: id(&format!("/dut/{name}/health_measure_cycle"))?,
                position_known: id(&format!("/dut/{name}/position_known"))?,
                health_known: id(&format!("/dut/{name}/health_known"))?,
            })
        };

        let mut actions = Vec::new();
        for name in ACTIONS {
            actions.push((*name, id(&format!("/actions/{name}"))?));
        }

        Ok(Self {
            ui_heartbeat: id("/ui/heartbeat")?,

            transport_desired: id("/transport/desired")?,
            transport_observed: id("/transport/observed")?,
            transport_port: id("/transport/port")?,
            transport_connected: id("/transport/connected")?,
            transport_detail: id("/transport/detail")?,

            serial: LinkParams {
                desired: id("/serial/desired")?,
                observed: id("/serial/observed")?,
                endpoint: id("/serial/port")?,
                connected: id("/serial/connected")?,
                detail: id("/serial/detail")?,
            },
            rs485: LinkParams {
                desired: id("/rs485/desired")?,
                observed: id("/rs485/observed")?,
                endpoint: id("/rs485/endpoint")?,
                connected: id("/rs485/connected")?,
                detail: id("/rs485/detail")?,
            },
            rs485_target: id("/rs485/target")?,
            rs485_discovered: id("/rs485/discovered")?,
            rs485_stats: Rs485StatsParams {
                tx: id("/rs485/stats/tx")?,
                rx: id("/rs485/stats/rx")?,
                acks: id("/rs485/stats/acks")?,
                ack_timeouts: id("/rs485/stats/ack_timeouts")?,
                decode_errors: id("/rs485/stats/decode_errors")?,
                outbox: id("/rs485/stats/outbox")?,
            },
            motion: MotionParams {
                route: id("/motion/route")?,
                a_rotations: id("/motion/a/rotations")?,
                b_rotations: id("/motion/b/rotations")?,
                max_velocity: id("/motion/profile/max_velocity")?,
                acceleration: id("/motion/profile/acceleration")?,
                min_velocity: id("/motion/profile/min_velocity")?,
            },
            test: TestParams {
                raw_vcom_text: id("/test/raw/vcom_text")?,
                raw_line_ending: id("/test/raw/line_ending")?,
                raw_rs485_json: id("/test/raw/rs485_json")?,
                current_a: id("/test/current_a")?,
                microstep: id("/test/microstep")?,
                home_threshold: id("/test/home_threshold")?,
                census_speed: id("/test/census_speed")?,
            },
            flash: FlashParams {
                auto_enabled: id("/flash/auto_enabled")?,
                armed: id("/flash/armed")?,
                busy: id("/flash/busy")?,
                phase: id("/flash/phase")?,
                step: id("/flash/step")?,
                progress: id("/flash/progress")?,
                detail: id("/flash/detail")?,
                boot_state: id("/flash/boot_state")?,
                boot_detail: id("/flash/boot_detail")?,
                needs_replug: id("/flash/needs_replug")?,
                last_outcome: id("/flash/last_outcome")?,
                boot_id: id("/flash/boot_id")?,
                app_id: id("/flash/app_id")?,
                scope: id("/flash/scope")?,
            },
            probe: ProbeParams {
                selected: id("/probe/selected")?,
                connected: id("/probe/connected")?,
                target_present: id("/probe/target_present")?,
                name: id("/probe/name")?,
                serial: id("/probe/serial")?,
                firmware: id("/probe/firmware")?,
                speed_khz: id("/probe/speed_khz")?,
            },
            mcu: McuParams {
                part: id("/mcu/part")?,
                uid: id("/mcu/uid")?,
                idcode: id("/mcu/idcode")?,
                dev_id: id("/mcu/dev_id")?,
                layout: id("/mcu/layout")?,
                rdp: id("/mcu/rdp")?,
                firmware: id("/mcu/firmware")?,
                flash_kb: id("/mcu/flash_kb")?,
            },

            dut_present: id("/dut/present")?,
            dut_firmware_kind: id("/dut/firmware_kind")?,
            dut_version: id("/dut/version")?,
            dut_uptime_s: id("/dut/uptime_s")?,
            dut_ratio: id("/dut/ratio")?,
            dut_usteps_per_rev: id("/dut/usteps_per_rev")?,
            axis_a: axis("a")?,
            axis_b: axis("b")?,

            threshold_floor: id("/dut/threshold/floor")?,
            threshold_band: id("/dut/threshold/band")?,
            threshold_applied: id("/dut/threshold/applied")?,
            threshold_calibrated_at_s: id("/dut/threshold/calibrated_at_s")?,

            plan_selected: id("/plan/selected")?,
            run_busy: id("/run/busy")?,
            run_plan: id("/run/plan")?,
            run_phase: id("/run/phase")?,
            run_origin: id("/run/origin")?,
            run_step_name: id("/run/step_name")?,
            run_step_index: id("/run/step_index")?,
            run_step_count: id("/run/step_count")?,
            run_step_fraction: id("/run/step_fraction")?,
            run_cycle: id("/run/cycle")?,
            run_cycle_count: id("/run/cycle_count")?,
            run_elapsed_s: id("/run/elapsed_s")?,
            run_eta_s: id("/run/eta_s")?,

            last_verdict: id("/last/verdict")?,
            last_plan: id("/last/plan")?,
            last_reason: id("/last/reason")?,
            last_advice: id("/last/advice")?,
            last_measurements: id("/last/measurements")?,
            last_report_path: id("/last/report_path")?,
            last_seq: id("/last/seq")?,

            counts_passed: id("/counts/passed")?,
            counts_failed: id("/counts/failed")?,
            counts_aborted: id("/counts/aborted")?,
            faults_active: id("/faults/active")?,

            cue: id("/cue")?,
            cue_seq: id("/cue_seq")?,

            setup_http_port: id("/setup/http_port")?,
            setup_simulated: id("/setup/simulated")?,
            setup_report_profile: id("/setup/report_profile")?,

            actions,
        })
    }
}

/// The action names, declared once so `declare` and `resolve` cannot drift apart.
pub const ACTIONS: &[&str] = &[
    "rescan",
    "connect_serial",
    "disconnect_serial",
    "identify_serial",
    "connect_rs485",
    "disconnect_rs485",
    "discover_rs485",
    "identify_rs485",
    "select_rs485_target",
    "connect",
    "disconnect",
    "identify",
    "poll",
    "run",
    "startup",
    "routine_startup",
    "abort",
    "escape",
    "calibrate_threshold",
    "calibrate_module",
    "home_a",
    "home_b",
    "unjam_a",
    "unjam_b",
    "backlash_a",
    "backlash_b",
    "reboot",
    "set_current",
    "set_microstep",
    "set_threshold",
    "census_a",
    "census_b",
    "send_raw",
    "flash",
    "flash_now",
    "reset_mcu",
    "check_boot",
    "read_device",
    "rescan_firmware",
    "motion_push",
    "marker",
];

/// Read an `i64` counter without caring whether it has ever been written.
pub fn get_i64(bus: &Bus, id: ParamId) -> i64 {
    match bus.get(id) {
        Some(Value::I64(value)) => value,
        _ => 0,
    }
}

pub fn get_text(bus: &Bus, id: ParamId) -> String {
    // `text` hands out a borrow rather than cloning, so the copy happens here and only here.
    bus.text(id, |s| s.to_string()).unwrap_or_default()
}

pub fn get_u32(bus: &Bus, id: ParamId) -> u32 {
    match bus.get(id) {
        Some(Value::Enum(value)) => value,
        Some(Value::I32(value)) => value as u32,
        _ => 0,
    }
}

pub fn get_i32(bus: &Bus, id: ParamId) -> i32 {
    match bus.get(id) {
        Some(Value::I32(value)) => value,
        _ => 0,
    }
}

pub fn get_f64(bus: &Bus, id: ParamId) -> f64 {
    match bus.get(id) {
        Some(Value::F64(value)) => value,
        _ => 0.0,
    }
}

pub fn get_bool(bus: &Bus, id: ParamId) -> bool {
    matches!(bus.get(id), Some(Value::Bool(true)))
}

/// Publish the facts about this process that never change after start.
pub fn publish_setup(bus: &Bus, params: &Params, port: u16, simulated: bool) -> Result<(), String> {
    let set = |id: ParamId, value: Value| bus.set(id, value).map_err(|e| e.to_string());
    set(params.setup_http_port, Value::I32(i32::from(port)))?;
    set(params.setup_simulated, Value::Bool(simulated))?;
    bus.set_text(params.setup_report_profile, bench_core::REPORT_PROFILE)
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// The operator-facing label for an action.
fn action_label(name: &str) -> String {
    match name {
        "rescan" => "Rescan ports and probes".into(),
        "connect_serial" => "Connect serial".into(),
        "disconnect_serial" => "Disconnect serial".into(),
        "identify_serial" => "Identify over serial".into(),
        "connect_rs485" => "Connect RS485".into(),
        "disconnect_rs485" => "Disconnect RS485".into(),
        "discover_rs485" => "Discover RS485 devices".into(),
        "identify_rs485" => "Identify RS485 target".into(),
        "select_rs485_target" => "Select RS485 target".into(),
        "connect" => "Connect".into(),
        "disconnect" => "Disconnect".into(),
        "identify" => "Identify module".into(),
        "poll" => "Poll module".into(),
        "run" => "Run plan".into(),
        "startup" => "Run startup test".into(),
        "routine_startup" => "Start module routine".into(),
        "abort" => "Abort".into(),
        "escape" => "Escape from routine".into(),
        "calibrate_threshold" => "Calibrate threshold".into(),
        "calibrate_module" => "Calibrate module".into(),
        "home_a" => "Home A".into(),
        "home_b" => "Home B".into(),
        "unjam_a" => "Unjam A".into(),
        "unjam_b" => "Unjam B".into(),
        "backlash_a" => "Measure backlash A".into(),
        "backlash_b" => "Measure backlash B".into(),
        "reboot" => "Reboot module".into(),
        "set_current" => "Set motor current".into(),
        "set_microstep" => "Set microstep resolution".into(),
        "set_threshold" => "Set home threshold".into(),
        "census_a" => "Run census A".into(),
        "census_b" => "Run census B".into(),
        "send_raw" => "Send raw signal".into(),
        "flash" => "Flash firmware".into(),
        "flash_now" => "Flash now".into(),
        "reset_mcu" => "Reset and run MCU".into(),
        "check_boot" => "Check MCU boot state".into(),
        "read_device" => "Read device".into(),
        "rescan_firmware" => "Rescan probes and firmware".into(),
        "motion_push" => "Push motion target".into(),
        "marker" => "Drop a marker".into(),
        other => other.replace('_', " "),
    }
}
