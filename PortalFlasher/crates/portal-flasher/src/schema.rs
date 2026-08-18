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

use av_gui_bus::{Bus, ParamFlags, ParamId, SchemaBuilder, Value};
use portal_swd::{Cue, Layout, Pass, Phase};

/// How many probe slots the schema declares. A bench has one; four is enough that a second
/// ST-Link plugged in by mistake is *visible* rather than silently ignored.
pub const PROBE_SLOTS: usize = 4;

/// How many firmware artefacts the schema can advertise. Two built plus a reference is three
/// today; six leaves room for extra reference images without a schema change.
pub const ARTEFACT_SLOTS: usize = 6;

/// What the probe and artefact slot parameters carry.
///
/// These 52 declarations are **backing store for two lists the page already draws properly**. They
/// exist so `ProbeRow` and `ArtefactRow` can subscribe per field; reading them one at a time is
/// never the right thing to do, and they were more than half of everything a generated parameter
/// view showed — which is most of why that view looked like a drawer of hidden settings.
///
/// `ADVANCED` is a hint to *generated* UI only. `ParamTree` hides them unless asked, and the
/// command palette demotes rather than hides them. It is not a wire-visibility bit and it is not a
/// permission: the declarations still ship, the values still stream, and the explicit `useParam`
/// subscriptions in the two row components are completely unaffected. The lists still work.
///
/// **`ParamBuilder::flags` replaces rather than unions**, unlike `read_only()` which unions. So
/// `READ_ONLY` has to be spelled here — writing `.read_only().flags(ADVANCED)` would silently drop
/// it and make all 52 client-writable, and a loop makes that mistake uniform.
const SLOT_FLAGS: ParamFlags = ParamFlags::READ_ONLY.union(ParamFlags::ADVANCED);

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

/// The stages of a flash pass, in the order they happen.
///
/// `idle` is index 0 so a session that connects between passes reads the right thing rather than
/// a stale `reset-run` from the last board. The rest mirror `portal_swd::Step` exactly; the
/// mapping in `worker.rs` is exhaustive so a new variant there cannot silently go unreported.
pub const STEPS: &[(u32, &str)] = &[
    (0, "idle"),
    (1, "attach"),
    (2, "option-bytes"),
    (3, "erase"),
    (4, "program"),
    (5, "readback"),
    (6, "reset-run"),
];

/// The result of the last completed pass. `none` until one has run.
pub const OUTCOMES: &[(u32, &str)] = &[(0, "none"), (1, "pass"), (2, "fail")];

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

    // A flash pass is the one operation long enough for `busy` alone to look like a hang: a chip
    // erase plus 50 kB of programming plus a 128 kB readback is seconds, not milliseconds. These
    // two say which of those is happening and how far in.
    check(
        builder
            .param("/rig/step")
            .enumeration(0, STEPS)
            .label("Step")
            .read_only()
            .register(),
    );
    check(
        builder
            .param("/rig/step_fraction")
            .f64(0.0)
            .range(0.0, 1.0)
            .label("Step progress")
            .read_only()
            .register(),
    );

    // ---------------------------------------------------------------- how the rig is set up
    //
    // Read-only, every one of them. Nothing here becomes configurable -- these exist because the
    // facts were true and *invisible*, which is what made a raw parameter dump at the bottom of the
    // page look like a drawer of hidden settings.
    //
    // Two of them are worth the whole group on their own. The SWD clock was on no parameter at all,
    // so an operator could not see what rate their probe had actually settled on. And the
    // option-byte policy writes option flash whenever the masked bits differ from the golden value
    // -- true on every pass, and previously announced only by `/rig/step` flicking past
    // `option-bytes` on its way to the erase.
    //
    // One prefix, so the page draws the lot with `<ParamTree prefix="/setup"/>` and no bespoke rows.
    check(
        builder
            .param("/setup/target")
            .text("")
            .label("Target")
            .read_only()
            .register(),
    );
    // Starts empty rather than at 1800. The configured rate is a request; `set_speed` answers with
    // what the probe actually applied, and until one is open this parameter would otherwise be
    // claiming a clock for hardware that is not attached.
    check(
        builder
            .param("/setup/swd_khz")
            .i32(0)
            .unit_label("kHz")
            .label("SWD clock")
            .read_only()
            .register(),
    );
    check(
        builder
            .param("/setup/erase")
            .text("")
            .label("Erase")
            .read_only()
            .register(),
    );
    check(
        builder
            .param("/setup/verify")
            .text("")
            .label("Verify")
            .read_only()
            .register(),
    );
    check(
        builder
            .param("/setup/option_bytes")
            .text("")
            .label("Option bytes")
            .read_only()
            .register(),
    );
    check(
        builder
            .param("/setup/debounce")
            .text("")
            .label("Debounce")
            .read_only()
            .register(),
    );
    check(
        builder
            .param("/setup/removal_gate_ms")
            .i32(0)
            .unit_label("ms")
            .label("Removal gate")
            .read_only()
            .register(),
    );
    check(
        builder
            .param("/setup/heartbeat_stale_ms")
            .i32(0)
            .unit_label("ms")
            .label("Disarms without a page after")
            .read_only()
            .register(),
    );
    // Answers "where did it look for firmware?", which `/image/hint` never did -- it says what is
    // missing without saying where it expected to find it.
    check(
        builder
            .param("/setup/firmware_root")
            .text("")
            .label("Firmware searched in")
            .read_only()
            .register(),
    );

    // ---------------------------------------------------------------- the last pass
    //
    // `/rig/detail` is a scratchpad: the probe-reopen path, `Read device` and the poll all
    // overwrite it. That was survivable while it only ever carried "no probe", and became a real
    // failure the first time a flash was attempted on hardware -- the pass failed, the fail tone
    // played, and the reason was gone from the screen within a tick, because a failed flash often
    // drops the probe and the reopen that follows clears the line.
    //
    // These are a *record*. Nothing clears them but the start of the next pass.
    check(
        builder
            .param("/last/outcome")
            .enumeration(0, OUTCOMES)
            .label("Last pass")
            .read_only()
            .register(),
    );
    check(
        builder
            .param("/last/detail")
            .text("")
            .label("What happened")
            .read_only()
            .register(),
    );
    // The `RigErrorKind`, by name. Coarser than the detail and much more stable, so it is what a
    // log greps for and what the advice below is keyed on.
    check(
        builder
            .param("/last/kind")
            .text("")
            .label("Kind")
            .read_only()
            .register(),
    );
    // How far it got. `attach` and `erase` are the difference between a board that was never
    // touched and one that must not leave the bench.
    check(
        builder
            .param("/last/step")
            .enumeration(0, STEPS)
            .label("Reached")
            .read_only()
            .register(),
    );
    check(
        builder
            .param("/last/advice")
            .text("")
            .label("What to do")
            .read_only()
            .register(),
    );
    // Whether flash contents may have changed before it failed. The single most useful bit at the
    // moment of a failure: try again, or quarantine the board.
    check(
        builder
            .param("/last/may_have_written")
            .bool(false)
            .label("Board may be half-written")
            .read_only()
            .register(),
    );
    // Monotonic, so a page can tell a fresh result from a repaint of the same one.
    check(
        builder
            .param("/last/seq")
            .i64(0)
            .label("Result sequence")
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
        for (leaf, label) in [
            ("id", "Id"),
            ("name", "Name"),
            ("serial", "Serial"),
            ("kind", "Kind"),
        ] {
            check(
                builder
                    .param(&format!("/probe/{slot:02}/{leaf}"))
                    .text("")
                    .label(label)
                    .flags(SLOT_FLAGS)
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
    //
    // The *scope* is not a separate control. It is which regions were chosen, so the two can
    // never disagree: pick both and it is a full image, pick one and the other bank is left
    // erased -- which is what a mass erase then genuinely does.
    check(
        builder
            .param("/image/boot_id")
            .text("")
            .label("Bootloader")
            .register(),
    );
    check(
        builder
            .param("/image/app_id")
            .text("")
            .label("Application")
            .register(),
    );
    check(
        builder
            .param("/image/count")
            .i32(0)
            .label("Artefacts")
            .read_only()
            .register(),
    );
    for slot in 0..ARTEFACT_SLOTS {
        for (leaf, label) in [
            ("id", "Id"),
            ("label", "Label"),
            ("region", "Region"),
            ("origin", "Origin"),
            ("detail", "Detail"),
        ] {
            check(
                builder
                    .param(&format!("/image/{slot:02}/{leaf}"))
                    .text("")
                    .label(label)
                    .flags(SLOT_FLAGS)
                    .register(),
            );
        }
        check(
            builder
                .param(&format!("/image/{slot:02}/fits"))
                .bool(false)
                .label("Fits")
                .flags(SLOT_FLAGS)
                .register(),
        );
    }
    for (path, label) in [
        ("/image/name", "Image"),
        ("/image/source", "Source"),
        ("/image/scope", "Scope"),
        ("/image/build_id", "Build"),
        ("/image/boot_sha", "Bootloader SHA-256"),
        ("/image/app_sha", "Application SHA-256"),
        // What is missing and how to produce it. A fresh clone has never built PortalFW, and
        // "run `pio run -e application_bank_optical`" is more use than an empty list.
        ("/image/hint", "Hint"),
        // Whether this image can be *proved* to run after it is flashed. Absent is a legitimate
        // state, not a fault -- but the operator should learn it before arming auto-flash, not
        // from a run-check that fails on a board that is actually fine.
        ("/image/run_check", "Run-check"),
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
        check(
            builder
                .param(path)
                .text("")
                .label(label)
                .read_only()
                .register(),
        );
    }
    for (path, label) in [
        ("/device/programmed_bytes", "Programmed"),
        ("/device/rdp_level", "RDP level"),
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

    // ---------------------------------------------------------------- the tally
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
    pub step: ParamId,
    pub step_fraction: ParamId,
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

    pub image_boot_id: ParamId,
    pub image_app_id: ParamId,
    pub image_count: ParamId,
    /// `(id, label, region, origin, detail, fits)` per slot.
    pub image_slots: Vec<(ParamId, ParamId, ParamId, ParamId, ParamId, ParamId)>,
    pub image_scope: ParamId,
    pub image_hint: ParamId,
    pub image_run_check: ParamId,

    pub setup_target: ParamId,
    pub setup_swd_khz: ParamId,
    pub setup_erase: ParamId,
    pub setup_verify: ParamId,
    pub setup_option_bytes: ParamId,
    pub setup_debounce: ParamId,
    pub setup_removal_gate_ms: ParamId,
    pub setup_heartbeat_stale_ms: ParamId,
    pub setup_firmware_root: ParamId,

    pub last_outcome: ParamId,
    pub last_detail: ParamId,
    pub last_kind: ParamId,
    pub last_step: ParamId,
    pub last_advice: ParamId,
    pub last_may_have_written: ParamId,
    pub last_seq: ParamId,
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
        let mut image_slots = Vec::with_capacity(ARTEFACT_SLOTS);
        for slot in 0..ARTEFACT_SLOTS {
            image_slots.push((
                id(&format!("/image/{slot:02}/id"))?,
                id(&format!("/image/{slot:02}/label"))?,
                id(&format!("/image/{slot:02}/region"))?,
                id(&format!("/image/{slot:02}/origin"))?,
                id(&format!("/image/{slot:02}/detail"))?,
                id(&format!("/image/{slot:02}/fits"))?,
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
            step: id("/rig/step")?,
            step_fraction: id("/rig/step_fraction")?,
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

            image_boot_id: id("/image/boot_id")?,
            image_app_id: id("/image/app_id")?,
            image_count: id("/image/count")?,
            image_slots,
            image_scope: id("/image/scope")?,
            image_hint: id("/image/hint")?,
            image_run_check: id("/image/run_check")?,

            setup_target: id("/setup/target")?,
            setup_swd_khz: id("/setup/swd_khz")?,
            setup_erase: id("/setup/erase")?,
            setup_verify: id("/setup/verify")?,
            setup_option_bytes: id("/setup/option_bytes")?,
            setup_debounce: id("/setup/debounce")?,
            setup_removal_gate_ms: id("/setup/removal_gate_ms")?,
            setup_heartbeat_stale_ms: id("/setup/heartbeat_stale_ms")?,
            setup_firmware_root: id("/setup/firmware_root")?,

            last_outcome: id("/last/outcome")?,
            last_detail: id("/last/detail")?,
            last_kind: id("/last/kind")?,
            last_step: id("/last/step")?,
            last_advice: id("/last/advice")?,
            last_may_have_written: id("/last/may_have_written")?,
            last_seq: id("/last/seq")?,
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

/// Only the tests read an i32 back off the bus -- the worker writes them and never re-reads.
#[cfg(test)]
pub fn get_i32(bus: &Bus, id: ParamId) -> i32 {
    match bus.get(id) {
        Some(Value::I32(v)) => v,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sealed() -> Bus {
        let mut builder = SchemaBuilder::new();
        declare(&mut builder, true).expect("schema");
        builder.seal()
    }

    /// The trap `SLOT_FLAGS` exists to avoid.
    ///
    /// `ParamBuilder::flags` *replaces* the flag set where `read_only()` unions into it, so adding
    /// `ADVANCED` the obvious way drops `READ_ONLY` — and these are declared in loops, so the
    /// mistake would land on all 52 at once and make a client able to write `/probe/00/serial`.
    #[test]
    fn every_slot_parameter_is_still_read_only() {
        let bus = sealed();
        let slots: Vec<_> = bus
            .schema()
            .params()
            .iter()
            .filter(|p| {
                p.path.starts_with("/probe/0") && p.path.matches('/').count() == 3
                    || p.path.starts_with("/image/0") && p.path.matches('/').count() == 3
            })
            .collect();

        assert_eq!(
            slots.len(),
            PROBE_SLOTS * 4 + ARTEFACT_SLOTS * 6,
            "the filter should match every slot parameter and nothing else"
        );
        for p in slots {
            assert!(
                p.flags.contains(av_gui_bus::ParamFlags::READ_ONLY),
                "{} lost READ_ONLY",
                p.path
            );
            assert!(
                p.flags.contains(av_gui_bus::ParamFlags::ADVANCED),
                "{} is not hidden from generated views",
                p.path
            );
        }
    }

    /// What a generated parameter view actually shows.
    ///
    /// The point of the change is the size of that view: every one of the 52 hidden declarations is
    /// list backing-store, and hiding exactly those and nothing else is the claim. Asserting the
    /// difference rather than a bare count means adding a real parameter later does not fail this.
    #[test]
    fn hiding_the_slots_removes_exactly_the_slots() {
        let bus = sealed();
        let total = bus.schema().params().len();
        let shown = bus
            .schema()
            .params()
            .iter()
            .filter(|p| !p.flags.contains(av_gui_bus::ParamFlags::ADVANCED))
            .count();

        assert_eq!(
            total - shown,
            PROBE_SLOTS * 4 + ARTEFACT_SLOTS * 6,
            "only the probe and artefact slot backing-store should be hidden"
        );
        // Every parameter an operator could act on survives the filter, which is the half of this
        // that would actually hurt to get wrong.
        for p in bus.schema().params() {
            if !p.flags.contains(av_gui_bus::ParamFlags::READ_ONLY) {
                assert!(
                    !p.flags.contains(av_gui_bus::ParamFlags::ADVANCED),
                    "{} is writable and hidden from generated views",
                    p.path
                );
            }
        }
    }

    /// Nothing in `/setup` is settable. It is a description of how the rig is configured, and a
    /// writable one would be a control that quietly changes how a board boots.
    #[test]
    fn the_setup_group_is_entirely_read_only() {
        let bus = sealed();
        let setup: Vec<_> = bus
            .schema()
            .params()
            .iter()
            .filter(|p| p.path.starts_with("/setup/"))
            .collect();

        assert!(!setup.is_empty(), "the setup group should exist");
        for p in setup {
            assert!(
                p.flags.contains(av_gui_bus::ParamFlags::READ_ONLY),
                "{} is writable; nothing in /setup may be",
                p.path
            );
        }
    }
}
