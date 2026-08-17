//! The parameter tree, and the ids the worker writes through.
//!
//! Desired and observed state are separate throughout — `/mode/desired` is what the operator
//! asked for, `/mode/observed` is what the worker actually holds. They disagree for a whole pass
//! every time a mode change arrives mid-write, and drawing that disagreement is the point.
//!
//! # What is a parameter and what is not
//!
//! Scalars an operator reads or sets live here. The device report does not: it is a layout enum,
//! two banners, a decoded option-byte word and 256 occupancy buckets, which is structured state
//! rather than a control surface. That travels over `GET /api/flasher/device`, the way the
//! framework's own router admin page carries its log and traffic ring off the bus.
//!
//! # Actions are counters
//!
//! "Rescan" and "Read device" bump an `i64` rather than toggling a flag, following
//! `example-vision`: *"'try that again' is a request and `Open` is a state."* The worker acts on
//! the change, so a repeated press works and a reconnecting page does not re-trigger anything.

use av_gui_bus::{Bus, ParamId, SchemaBuilder, Value};
use portal_swd::{Cue, Layout, Pass, Phase};

/// How many probe slots the schema declares. A bench has one; four is enough that a second
/// ST-Link plugged in by mistake is *visible* rather than silently ignored.
pub const PROBE_SLOTS: usize = 4;

/// Enum variants, declared once and read back by name on the page. Never by discriminant: a page
/// keyed on `2` inverts silently the moment someone reorders this list.
pub const MODES: &[(u32, &str)] = &[(0, "manual"), (1, "auto")];

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

pub const LAYOUTS: &[(u32, &str)] = &[
    (0, "unknown"),
    (1, "erased"),
    (2, "split"),
    (3, "flat"),
    (4, "unrecognised"),
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

pub fn layout_index(layout: Option<Layout>) -> u32 {
    match layout {
        None => 0,
        Some(Layout::Erased) => 1,
        Some(Layout::Split) => 2,
        Some(Layout::Flat) => 3,
        Some(Layout::Unrecognised) => 4,
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

    // ---------------------------------------------------------------- mode
    //
    // "Armed" described the wrong thing. The rig has a mode: manual is the default and every
    // flash is a deliberate press; auto-flash is the hands-free rhythm, and *that* is what gets
    // armed.
    check(
        builder
            .param("/mode/desired")
            .enumeration(0, MODES)
            .label("Mode")
            .register(),
    );
    check(
        builder
            .param("/mode/observed")
            .enumeration(0, MODES)
            .label("Mode")
            .read_only()
            .register(),
    );
    check(
        builder
            .param("/autoflash/armed")
            .bool(false)
            .label("Armed")
            .read_only()
            .register(),
    );
    // The dead man. The page re-asserts this about once a second; the worker drops out of
    // auto-flash if it goes stale. Sound lives in the browser, so a rig nobody can hear must not
    // stay armed.
    check(
        builder
            .param("/ui/heartbeat")
            .i64(0)
            .label("UI heartbeat")
            .register(),
    );

    // ---------------------------------------------------------------- the rig
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
    check(
        builder
            .param("/rig/busy")
            .bool(false)
            .label("Busy")
            .read_only()
            .register(),
    );

    // A cue is an event, not a state, so it travels as a value plus a monotonic sequence. A
    // session that connects late sees the last cue without hearing it again.
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

    // ---------------------------------------------------------------- probes
    //
    // Selecting writes the parameter and does nothing else; connecting is the worker's business.
    // `example-vision`'s rule, and for the same reason: a click that seized a probe would take it
    // from whatever else on the bench is using it.
    check(
        builder
            .param("/probe/selected")
            .text("")
            .label("Probe")
            .register(),
    );
    check(
        builder
            .param("/probe/count")
            .i32(0)
            .label("Probes")
            .read_only()
            .register(),
    );
    check(
        builder
            .param("/probe/connected")
            .bool(false)
            .label("Connected")
            .read_only()
            .register(),
    );
    // Whether the last poll saw a target. Published rather than left for the page to infer from
    // the phase: in manual mode the phase never leaves `disarmed`, so inferring it meant Read
    // device stayed disabled until something had been read — which nothing could be.
    check(
        builder
            .param("/probe/target_present")
            .bool(false)
            .label("Board")
            .read_only()
            .register(),
    );
    for slot in 0..PROBE_SLOTS {
        for (leaf, label) in [("id", "Id"), ("name", "Name"), ("serial", "Serial"), ("kind", "Kind")]
        {
            check(
                builder
                    .param(&format!("/probe/{slot:02}/{leaf}"))
                    .text("")
                    .label(label)
                    .read_only()
                    .register(),
            );
        }
    }

    // ---------------------------------------------------------------- actions
    for (path, label) in [
        ("/actions/rescan", "Rescan probes"),
        ("/actions/read_device", "Read device"),
        ("/actions/flash_now", "Flash now"),
    ] {
        check(builder.param(path).i64(0).label(label).register());
    }

    // ---------------------------------------------------------------- the image
    for (path, label) in [
        ("/image/name", "Image"),
        ("/image/source", "Source"),
        ("/image/build_id", "Build"),
        ("/image/boot_sha", "Bootloader SHA-256"),
        ("/image/app_sha", "Application SHA-256"),
    ] {
        check(builder.param(path).text("").label(label).read_only().register());
    }

    // ---------------------------------------------------------------- the device
    //
    // Summary scalars only. The full report -- occupancy buckets, per-region detail, decoded
    // option bytes -- is `GET /api/flasher/device`, because 256 buckets is not a control.
    check(
        builder
            .param("/device/read")
            .bool(false)
            .label("Read")
            .read_only()
            .register(),
    );
    check(
        builder
            .param("/device/layout")
            .enumeration(0, LAYOUTS)
            .label("Layout")
            .read_only()
            .register(),
    );
    for (path, label) in [
        ("/device/uid", "UID"),
        ("/device/banner", "Firmware"),
        ("/device/warnings", "Warnings"),
    ] {
        check(builder.param(path).text("").label(label).read_only().register());
    }
    for (path, label) in [
        ("/device/programmed_bytes", "Programmed"),
        ("/device/rdp_level", "RDP level"),
    ] {
        check(builder.param(path).i32(0).label(label).read_only().register());
    }

    // ---------------------------------------------------------------- the tally
    for (path, label) in [
        ("/counts/passed", "Passed"),
        ("/counts/failed", "Failed"),
        ("/faults/active", "Faults"),
    ] {
        check(builder.param(path).i32(0).label(label).read_only().register());
    }

    if simulated {
        // Only under `--simulate`, so the product schema never carries a control that does
        // nothing on real hardware. This is the fixture: what a board being seated and lifted
        // looks like to the poll.
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
#[derive(Clone, Debug)]
pub struct Params {
    pub mode_desired: ParamId,
    pub mode_observed: ParamId,
    pub autoflash_armed: ParamId,
    pub heartbeat: ParamId,

    pub phase: ParamId,
    pub expect: ParamId,
    pub detail: ParamId,
    pub busy: ParamId,
    pub cue: ParamId,
    pub cue_seq: ParamId,

    pub probe_selected: ParamId,
    pub probe_count: ParamId,
    pub probe_connected: ParamId,
    pub probe_target_present: ParamId,
    /// `(id, name, serial, kind)` per slot.
    pub probe_slots: Vec<(ParamId, ParamId, ParamId, ParamId)>,

    pub act_rescan: ParamId,
    pub act_read_device: ParamId,
    pub act_flash_now: ParamId,

    pub image_name: ParamId,
    pub image_source: ParamId,
    pub image_build_id: ParamId,
    pub image_boot_sha: ParamId,
    pub image_app_sha: ParamId,

    pub device_read: ParamId,
    pub device_layout: ParamId,
    pub device_uid: ParamId,
    pub device_banner: ParamId,
    pub device_warnings: ParamId,
    pub device_programmed: ParamId,
    pub device_rdp: ParamId,

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
        let mut probe_slots = Vec::with_capacity(PROBE_SLOTS);
        for slot in 0..PROBE_SLOTS {
            probe_slots.push((
                id(&format!("/probe/{slot:02}/id"))?,
                id(&format!("/probe/{slot:02}/name"))?,
                id(&format!("/probe/{slot:02}/serial"))?,
                id(&format!("/probe/{slot:02}/kind"))?,
            ));
        }
        Ok(Self {
            mode_desired: id("/mode/desired")?,
            mode_observed: id("/mode/observed")?,
            autoflash_armed: id("/autoflash/armed")?,
            heartbeat: id("/ui/heartbeat")?,

            phase: id("/rig/phase")?,
            expect: id("/rig/expect")?,
            detail: id("/rig/detail")?,
            busy: id("/rig/busy")?,
            cue: id("/rig/cue")?,
            cue_seq: id("/rig/cue_seq")?,

            probe_selected: id("/probe/selected")?,
            probe_count: id("/probe/count")?,
            probe_connected: id("/probe/connected")?,
            probe_target_present: id("/probe/target_present")?,
            probe_slots,

            act_rescan: id("/actions/rescan")?,
            act_read_device: id("/actions/read_device")?,
            act_flash_now: id("/actions/flash_now")?,

            image_name: id("/image/name")?,
            image_source: id("/image/source")?,
            image_build_id: id("/image/build_id")?,
            image_boot_sha: id("/image/boot_sha")?,
            image_app_sha: id("/image/app_sha")?,

            device_read: id("/device/read")?,
            device_layout: id("/device/layout")?,
            device_uid: id("/device/uid")?,
            device_banner: id("/device/banner")?,
            device_warnings: id("/device/warnings")?,
            device_programmed: id("/device/programmed_bytes")?,
            device_rdp: id("/device/rdp_level")?,

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

pub fn get_enum(bus: &Bus, id: ParamId) -> u32 {
    match bus.get(id) {
        Some(Value::Enum(v)) => v,
        _ => 0,
    }
}

pub fn get_text(bus: &Bus, id: ParamId) -> String {
    bus.text(id, |s| s.to_owned()).unwrap_or_default()
}
