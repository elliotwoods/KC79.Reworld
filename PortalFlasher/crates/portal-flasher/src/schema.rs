//! The parameter tree, and the ids the worker writes through.
//!
//! Desired and observed state are separate parameters throughout — `/arm/desired` is what the
//! operator asked for, `/arm/observed` is what the worker actually holds. They disagree for a
//! whole pass every time a disarm arrives mid-write, and drawing that disagreement is the point.

use av_gui_bus::{Bus, ParamId, SchemaBuilder, Value};
use portal_swd::{Cue, Pass, Phase};

/// Enum variants, declared once and read back by name on the page. Never by discriminant: a
/// page keyed on `2` inverts silently the moment someone reorders this list.
pub const PHASES: &[(u32, &str)] = &[
    (0, "disarmed"),
    (1, "idle"),
    (2, "debouncing"),
    (3, "flashing"),
    (4, "run-check"),
    (5, "await-removal"),
    (6, "probe-lost"),
];

pub const EXPECTS: &[(u32, &str)] = &[(0, "flash"), (1, "run-check")];

pub const CUES: &[(u32, &str)] = &[
    (0, "none"),
    (1, "armed"),
    (2, "disarmed"),
    (3, "busy"),
    (4, "flashed-cycle-it"),
    (5, "pass"),
    (6, "fail"),
    (7, "rearmed"),
];

pub fn phase_index(phase: Phase) -> u32 {
    match phase {
        Phase::Disarmed => 0,
        Phase::Idle => 1,
        Phase::Debouncing => 2,
        Phase::Flashing => 3,
        Phase::RunChecking => 4,
        Phase::AwaitRemoval => 5,
        Phase::ProbeLost => 6,
    }
}

pub fn expect_index(pass: Option<Pass>) -> u32 {
    match pass {
        Some(Pass::RunCheck) => 1,
        _ => 0,
    }
}

pub fn cue_index(cue: Cue) -> u32 {
    match cue {
        Cue::Armed => 1,
        Cue::Disarmed => 2,
        Cue::Busy => 3,
        Cue::FlashedCycleIt => 4,
        Cue::Pass => 5,
        Cue::Fail => 6,
        Cue::Rearmed => 7,
    }
}

pub fn declare(builder: &mut SchemaBuilder, simulated: bool) -> Result<(), String> {
    let mut err = None;
    let mut check = |result: Result<ParamId, av_gui_bus::BusError>| {
        if let Err(e) = result
            && err.is_none()
        {
            err = Some(format!("{e:?}"));
        }
    };

    // ---- what the operator asks for
    check(
        builder
            .param("/arm/desired")
            .bool(false)
            .label("Arm")
            .register(),
    );
    // The dead man. The page re-asserts this about once a second; the worker disarms if it goes
    // stale. Sound lives in the browser, so a rig nobody can hear must not stay armed.
    check(
        builder
            .param("/arm/heartbeat")
            .i64(0)
            .label("UI heartbeat")
            .register(),
    );

    // ---- what is actually happening
    check(
        builder
            .param("/arm/observed")
            .bool(false)
            .label("Armed")
            .read_only()
            .register(),
    );
    check(
        builder
            .param("/rig/phase")
            .enumeration(0, PHASES)
            .label("Phase")
            .read_only()
            .register(),
    );
    check(
        builder
            .param("/rig/expect")
            .enumeration(0, EXPECTS)
            .label("Next pass")
            .read_only()
            .register(),
    );
    check(
        builder
            .param("/rig/detail")
            .text("")
            .label("Detail")
            .read_only()
            .register(),
    );

    // A cue is an event, not a state, so it travels as a value plus a monotonic sequence. The
    // page plays a sound when the sequence changes, which means a session that connects late
    // sees the last cue without hearing it again.
    check(
        builder
            .param("/rig/cue")
            .enumeration(0, CUES)
            .label("Cue")
            .read_only()
            .register(),
    );
    check(
        builder
            .param("/rig/cue_seq")
            .i64(0)
            .label("Cue sequence")
            .read_only()
            .register(),
    );

    // ---- the probe
    check(
        builder
            .param("/probe/present")
            .bool(false)
            .label("Probe")
            .read_only()
            .register(),
    );
    check(
        builder
            .param("/probe/name")
            .text("")
            .label("Probe name")
            .read_only()
            .register(),
    );

    // ---- the image
    for (path, label) in [
        ("/image/name", "Image"),
        ("/image/source", "Source"),
        ("/image/build_id", "Build"),
        ("/image/boot_sha", "Bootloader SHA-256"),
        ("/image/app_sha", "Application SHA-256"),
    ] {
        check(
            builder
                .param(path)
                .text("")
                .label(label)
                .read_only()
                .register(),
        );
    }

    // ---- the tally
    for (path, label) in [
        ("/counts/passed", "Passed"),
        ("/counts/failed", "Failed"),
        ("/faults/active", "Faults"),
    ] {
        check(
            builder
                .param(path)
                .i32(0)
                .label(label)
                .read_only()
                .register(),
        );
    }

    if simulated {
        // Only present under `--simulate`, so the product schema never carries a control that
        // does nothing on real hardware. This is the fixture, in effect: it is what a board
        // being seated and lifted looks like to the poll.
        check(
            builder
                .param("/sim/board_present")
                .bool(false)
                .label("Board in fixture")
                .register(),
        );
        check(
            builder
                .param("/sim/fail_next_pass")
                .bool(false)
                .label("Fail the next pass")
                .register(),
        );
    }

    match err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Resolved ids, so the worker does a string lookup once rather than every tick.
#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub arm_desired: ParamId,
    pub arm_heartbeat: ParamId,
    pub arm_observed: ParamId,
    pub phase: ParamId,
    pub expect: ParamId,
    pub detail: ParamId,
    pub cue: ParamId,
    pub cue_seq: ParamId,
    pub probe_present: ParamId,
    pub probe_name: ParamId,
    pub image_name: ParamId,
    pub image_source: ParamId,
    pub image_build_id: ParamId,
    pub image_boot_sha: ParamId,
    pub image_app_sha: ParamId,
    pub passed: ParamId,
    pub failed: ParamId,
    pub faults: ParamId,
    pub sim_board_present: Option<ParamId>,
    pub sim_fail_next: Option<ParamId>,
}

impl Params {
    pub fn resolve(bus: &Bus) -> Result<Self, String> {
        let id = |path: &str| {
            bus.id_of(path)
                .ok_or_else(|| format!("{path} is not in the sealed schema"))
        };
        Ok(Self {
            arm_desired: id("/arm/desired")?,
            arm_heartbeat: id("/arm/heartbeat")?,
            arm_observed: id("/arm/observed")?,
            phase: id("/rig/phase")?,
            expect: id("/rig/expect")?,
            detail: id("/rig/detail")?,
            cue: id("/rig/cue")?,
            cue_seq: id("/rig/cue_seq")?,
            probe_present: id("/probe/present")?,
            probe_name: id("/probe/name")?,
            image_name: id("/image/name")?,
            image_source: id("/image/source")?,
            image_build_id: id("/image/build_id")?,
            image_boot_sha: id("/image/boot_sha")?,
            image_app_sha: id("/image/app_sha")?,
            passed: id("/counts/passed")?,
            failed: id("/counts/failed")?,
            faults: id("/faults/active")?,
            sim_board_present: bus.id_of("/sim/board_present"),
            sim_fail_next: bus.id_of("/sim/fail_next_pass"),
        })
    }
}

/// Small helpers so the worker reads as what it means rather than as pattern matches.
pub fn get_bool(bus: &Bus, id: ParamId) -> bool {
    matches!(bus.get(id), Some(Value::Bool(true)))
}

pub fn get_i64(bus: &Bus, id: ParamId) -> i64 {
    match bus.get(id) {
        Some(Value::I64(v)) => v,
        _ => 0,
    }
}
