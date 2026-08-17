//! The one thread that owns the probe and runs the state machine.
//!
//! A plain OS thread, not a Tokio task, and deliberately so: it blocks for seconds at a time
//! inside a flash pass, and it must keep ticking whether or not anyone is looking at the UI. The
//! page is a view. Every decision is made here.
//!
//! # Two modes, one of which is armed
//!
//! **Manual** is the default: the operator picks firmware and presses Flash now. **Auto-flash**
//! is the hands-free rhythm — debounce, flash, cycle, run-check — and it is auto-flash that gets
//! armed. Only auto-flash feeds the state machine; a manual press is a direct action, because
//! the machine's debounce and removal gate exist for a rhythm a deliberate press is not part of.
//!
//! The framework's bus has no write-notification callback — an application core polls it, the way
//! `example-console`'s does. That suits this worker, which already has a clock of its own.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use av_gui_bus::{Bus, Value};
use portal_swd::{
    Action, Cue, ImageBundle, Input, Machine, Millis, Pass, Presence, Rig, RigError, RigErrorKind,
    Timing,
};

use crate::device_api::{DeviceJson, DeviceState};
use crate::schema::{self, Params};

pub struct Worker {
    bus: Arc<Bus>,
    params: Params,
    machine: Machine,
    rig: Box<dyn Rig>,
    bundle: Option<ImageBundle>,
    device: DeviceState,
    started: Instant,
    poll_period: Duration,
    cue_seq: i64,
    passed: i32,
    failed: i32,
    faults: i32,
    probe_open: bool,
    /// Last heartbeat value seen on the bus, to notice the page actually moving it.
    last_heartbeat_value: i64,
    /// Tracks the armed edge, so a self-disarm can retract the request that caused it.
    was_armed: bool,
    /// Action counters, so a repeated press works and a reconnecting page re-triggers nothing.
    last_rescan: i64,
    last_read: i64,
    last_flash: i64,
    probes: Vec<portal_swd::ProbeDescriptor>,
    discovery: portal_swd::Discovery,
    selection: portal_swd::artefacts::Selection,
    /// Under --simulate the bundle is synthetic and discovery has nothing to do with it.
    simulated: bool,
    /// Last selection seen on the bus, so a change reloads the bundle without polling the disk.
    last_selection: (String, String),
    /// Whether the most recent poll saw a target. Drives whether Flash now is offered.
    target_present: bool,
    /// Present only under `--simulate`: the shared fixture the page's switch drives.
    fixture: Option<Arc<AtomicBool>>,
}

impl Worker {
    pub fn new(
        bus: Arc<Bus>,
        params: Params,
        rig: Box<dyn Rig>,
        bundle: Option<ImageBundle>,
        device: DeviceState,
        simulated: bool,
        fixture: Option<Arc<AtomicBool>>,
    ) -> Self {
        let timing = Timing::default();
        let bundle_is_synthetic = simulated;
        Self {
            bus,
            params,
            machine: Machine::new(timing),
            rig,
            bundle,
            device,
            started: Instant::now(),
            poll_period: Duration::from_millis(timing.idle_poll_ms),
            cue_seq: 0,
            passed: 0,
            failed: 0,
            faults: 0,
            probe_open: false,
            last_heartbeat_value: 0,
            was_armed: false,
            last_rescan: 0,
            last_read: 0,
            last_flash: 0,
            probes: Vec::new(),
            discovery: portal_swd::Discovery::default(),
            selection: portal_swd::artefacts::Selection::default(),
            simulated: bundle_is_synthetic,
            last_selection: (String::new(), String::new()),
            target_present: false,
            fixture,
        }
    }

    fn now(&self) -> Millis {
        self.started.elapsed().as_millis() as Millis
    }

    pub fn run(mut self) {
        self.rediscover();
        self.publish_image();
        self.rescan();
        loop {
            std::thread::sleep(self.poll_period);
            let now = self.now();
            self.tick_at(now);
        }
    }

    /// One pass of the loop, at a caller-supplied instant.
    ///
    /// Split out from `run` so the dead-man can be tested in milliseconds rather than by waiting
    /// three real seconds.
    fn tick_at(&mut self, now: Millis) {
        // ---- the UI's own liveness, from two independent facts.
        //
        // `live_sessions` catches a closed tab; the heartbeat catches a page whose script has
        // wedged while its socket stays open. Either alone would miss one of them.
        let heartbeat = schema::get_i64(&self.bus, self.params.heartbeat);
        let page_moving = heartbeat != self.last_heartbeat_value;
        self.last_heartbeat_value = heartbeat;
        if page_moving && self.bus.live_sessions() > 0 {
            self.step(now, Input::Heartbeat);
        }

        // ---- mode. Only auto-flash arms the machine.
        let wants_auto = schema::get_enum(&self.bus, self.params.mode_desired) == 1;
        if wants_auto && !self.machine.armed() {
            self.step(now, Input::Arm);
        } else if !wants_auto && self.machine.armed() {
            self.step(now, Input::Disarm);
        }

        self.handle_actions(now);

        // ---- keep the probe open, and say so
        if !self.probe_open {
            match self.rig.open() {
                Ok(_info) => {
                    self.probe_open = true;
                    let _ = self.bus.set(self.params.probe_connected, Value::Bool(true));
                    self.set_detail("");
                    self.step(now, Input::ProbeRecovered);
                }
                Err(err) => {
                    let _ = self
                        .bus
                        .set(self.params.probe_connected, Value::Bool(false));
                    self.set_detail(&err.detail);
                    self.step(now, Input::ProbeError);
                }
            }
        }

        // ---- poll, unless a pass owns the probe
        //
        // The poll runs in both modes. Manual needs it too: Flash now should only be offered when
        // something is actually answering, and the page has no other way to know.
        if self.probe_open && !self.machine.pass_in_flight() {
            self.sync_simulation();
            match self.rig.poll() {
                Ok(Presence::Present) => {
                    self.target_present = true;
                    self.step(now, Input::PollPresent);
                }
                Ok(Presence::Absent) => {
                    self.target_present = false;
                    self.step(now, Input::PollAbsent);
                }
                Err(err) => {
                    self.target_present = false;
                    self.on_rig_error(now, err);
                }
            }
        }

        self.step(now, Input::Tick);
        // Last, deliberately. The machine can disarm itself part-way through this tick -- the
        // dead-man trips inside whichever `step` runs after the deadline, usually a poll -- and
        // the falling edge has to be observed after all of that. Checked before the polls, it
        // never sees the transition, and the next tick re-arms from a request nothing cleared.
        self.settle_mode();
        self.publish_state();
    }

    // ---------------------------------------------------------------- actions

    fn handle_actions(&mut self, now: Millis) {
        let rescan = schema::get_i64(&self.bus, self.params.act_rescan);
        if rescan != self.last_rescan {
            self.last_rescan = rescan;
            self.rescan();
            self.rediscover();
        }

        let read = schema::get_i64(&self.bus, self.params.act_read_device);
        if read != self.last_read {
            self.last_read = read;
            self.read_device(now);
        }

        // A selection change is a bus write with no counter, so it is noticed by comparison.
        let selection = (
            schema::get_text(&self.bus, self.params.image_boot_id),
            schema::get_text(&self.bus, self.params.image_app_id),
        );
        if selection != self.last_selection {
            self.last_selection = selection;
            self.reload_bundle();
        }

        let flash = schema::get_i64(&self.bus, self.params.act_flash_now);
        if flash != self.last_flash {
            self.last_flash = flash;
            self.flash_now(now);
        }
    }

    /// Re-enumerate, publish the list, and adopt the operator's selection.
    fn rescan(&mut self) {
        self.probes = portal_swd::list_probes();
        let _ = self.bus.set(
            self.params.probe_count,
            Value::I32(self.probes.len() as i32),
        );

        for (slot, (id, name, serial, kind)) in self.params.probe_slots.iter().enumerate() {
            let found = self.probes.get(slot);
            let _ = self.bus.set_text(*id, found.map_or("", |p| p.id.as_str()));
            let _ = self
                .bus
                .set_text(*name, found.map_or("", |p| p.name.as_str()));
            let _ = self.bus.set_text(
                *serial,
                found.and_then(|p| p.serial.as_deref()).unwrap_or(""),
            );
            let _ = self
                .bus
                .set_text(*kind, found.map_or("", |p| p.kind.as_str()));
        }

        // If nothing is selected yet, adopt the only probe there is. With more than one, leave it
        // unselected: guessing which ST-Link on a bench is the fixture is exactly the kind of
        // helpfulness that flashes the wrong board.
        let selected = schema::get_text(&self.bus, self.params.probe_selected);
        if selected.is_empty() && self.probes.len() == 1 {
            let _ = self
                .bus
                .set_text(self.params.probe_selected, &self.probes[0].id);
        }
    }

    fn read_device(&mut self, now: Millis) {
        if !self.probe_open {
            self.set_detail("no probe");
            return;
        }
        if self.machine.pass_in_flight() {
            self.set_detail("a pass is running");
            return;
        }

        match self.rig.read_device() {
            Ok(image) => {
                let report = image.analyse();
                let read_at_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or_default();
                let json = DeviceJson::build(&image, &report, self.bundle.as_ref(), read_at_ms);

                let p = &self.params;
                let _ = self.bus.set(p.device_read, Value::Bool(true));
                let _ = self.bus.set(
                    p.device_layout,
                    Value::Enum(schema::layout_index(Some(report.layout))),
                );
                let _ = self.bus.set_text(p.device_uid, &report.uid);
                let banner = report
                    .application
                    .banner
                    .clone()
                    .or_else(|| report.bootloader.banner.clone())
                    .unwrap_or_default();
                let _ = self.bus.set_text(p.device_banner, &banner);
                let warnings = report
                    .options
                    .warnings()
                    .iter()
                    .map(|w| w.to_string())
                    .collect::<Vec<_>>()
                    .join("; ");
                let _ = self.bus.set_text(p.device_warnings, &warnings);
                let _ = self.bus.set(
                    p.device_programmed,
                    Value::I32(report.programmed_bytes as i32),
                );
                let _ = self.bus.set(
                    p.device_rdp,
                    Value::I32(i32::from(report.options.rdp_level())),
                );
                self.set_detail("");

                if let Ok(mut guard) = self.device.lock() {
                    *guard = Some(json);
                }
            }
            Err(err) => {
                self.set_detail(&err.detail);
                self.faults += 1;
                if err.is_probe_loss() {
                    self.probe_open = false;
                    self.step(now, Input::ProbeError);
                }
            }
        }
    }

    /// A deliberate single flash of whatever is in the fixture.
    ///
    /// Not routed through the state machine: the debounce and the removal gate exist for a
    /// hands-free rhythm, and a button press is not that. Gating on "not armed" keeps the two
    /// paths from ever running at once.
    fn flash_now(&mut self, now: Millis) {
        if self.machine.armed() {
            self.set_detail("disengage auto-flash before flashing manually");
            return;
        }
        if !self.probe_open {
            self.set_detail("no probe");
            return;
        }
        if !self.target_present {
            self.set_detail("nothing is answering in the fixture");
            return;
        }
        let Some(bundle) = self.bundle.clone() else {
            self.set_detail("no image selected");
            return;
        };

        let _ = self.bus.set(self.params.busy, Value::Bool(true));
        self.emit_cue(Cue::Busy);
        let mut progress = |_step, _done, _total| {};
        let outcome = self.rig.flash(&bundle, &mut progress);
        let _ = self.bus.set(self.params.busy, Value::Bool(false));

        match outcome {
            Ok(_) => {
                self.set_detail("");
                self.passed += 1;
                self.emit_cue(Cue::Pass);
            }
            Err(err) => {
                self.set_detail(&err.detail);
                self.failed += 1;
                self.faults += 1;
                if err.is_probe_loss() {
                    self.probe_open = false;
                    self.step(now, Input::ProbeError);
                }
                self.emit_cue(Cue::Fail);
            }
        }
    }

    // ---------------------------------------------------------------- plumbing

    /// Retract the operator's request when the rig drops out of auto-flash on its own.
    ///
    /// Without this the dead-man does not work at all, and the failure is silent: the machine
    /// notices the page has gone and disarms, then the very next tick reads `/mode/desired` —
    /// still `auto`, because nothing changed it — and arms straight back up. Measured, not
    /// theorised: closing the tab left the rig armed with nobody able to hear it.
    fn settle_mode(&mut self) {
        let armed = self.machine.armed();
        if self.was_armed && !armed {
            let _ = self.bus.set(self.params.mode_desired, Value::Enum(0));
        }
        self.was_armed = armed;
    }

    fn sync_simulation(&mut self) {
        let (Some(id), Some(fixture)) = (self.params.sim_board_present, self.fixture.as_ref())
        else {
            return;
        };
        fixture.store(schema::get_bool(&self.bus, id), Ordering::Relaxed);
    }

    fn on_rig_error(&mut self, now: Millis, err: RigError) {
        self.set_detail(&err.detail);
        if err.is_probe_loss() {
            self.probe_open = false;
            let _ = self
                .bus
                .set(self.params.probe_connected, Value::Bool(false));
            self.step(now, Input::ProbeError);
        } else {
            // Not the probe: the target went away, which is an ordinary operator event and the
            // debounce's business rather than a fault.
            self.step(now, Input::PollAbsent);
        }
    }

    fn step(&mut self, now: Millis, input: Input) {
        for action in self.machine.step(now, input) {
            match action {
                Action::SetPollPeriod(ms) => self.poll_period = Duration::from_millis(ms),
                Action::Sound(cue) => self.emit_cue(cue),
                Action::BeginPass(pass) => self.run_pass(now, pass),
            }
        }
    }

    fn run_pass(&mut self, now: Millis, pass: Pass) {
        let Some(bundle) = self.bundle.clone() else {
            self.set_detail("no image loaded");
            self.faults += 1;
            self.step(now, Input::PassDone { pass, ok: false });
            return;
        };

        // One-shot failure injection, so an operator can hear the fail tone and watch the removal
        // gate without having to sabotage a board. Cleared as it is consumed.
        if let Some(id) = self.params.sim_fail_next
            && schema::get_bool(&self.bus, id)
        {
            let _ = self.bus.set(id, Value::Bool(false));
            self.set_detail("simulated failure");
            self.failed += 1;
            self.faults += 1;
            self.step(now, Input::PassDone { pass, ok: false });
            return;
        }

        let _ = self.bus.set(self.params.busy, Value::Bool(true));
        let outcome = match pass {
            Pass::Flash => {
                let mut progress = |_step, _done, _total| {};
                self.rig
                    .flash(&bundle, &mut progress)
                    .map(|report| report.readback_sha256)
            }
            Pass::RunCheck => self.rig.run_check(&bundle.run_check).and_then(|report| {
                report
                    .verdict(&bundle.run_check)
                    .map(|()| String::new())
                    .map_err(|fault| RigError::new(RigErrorKind::NotRunning, fault.to_string()))
            }),
        };
        let _ = self.bus.set(self.params.busy, Value::Bool(false));

        let ok = match outcome {
            Ok(_) => {
                self.set_detail("");
                true
            }
            Err(err) => {
                self.set_detail(&err.detail);
                if err.is_probe_loss() {
                    self.probe_open = false;
                }
                false
            }
        };

        if ok {
            // Only a completed run-check counts a board as done. A flash on its own is half a
            // board, and counting it would overstate the batch.
            if pass == Pass::RunCheck {
                self.passed += 1;
            }
        } else {
            self.failed += 1;
            self.faults += 1;
        }

        self.step(now, Input::PassDone { pass, ok });
    }

    fn emit_cue(&mut self, cue: Cue) {
        self.cue_seq += 1;
        let _ = self
            .bus
            .set(self.params.cue, Value::Enum(schema::cue_index(cue)));
        let _ = self.bus.set(self.params.cue_seq, Value::I64(self.cue_seq));
    }

    fn set_detail(&mut self, text: &str) {
        let _ = self.bus.set_text(self.params.detail, text);
    }

    fn publish_state(&self) {
        let p = &self.params;
        let armed = self.machine.armed();
        let _ = self.bus.set(p.autoflash_armed, Value::Bool(armed));
        let _ = self
            .bus
            .set(p.probe_target_present, Value::Bool(self.target_present));
        let _ = self.bus.set(p.mode_observed, Value::Enum(u32::from(armed)));
        let _ = self.bus.set(
            p.phase,
            Value::Enum(schema::phase_index(self.machine.phase())),
        );
        let _ = self.bus.set(
            p.expect,
            Value::Enum(schema::expect_index(self.machine.expect())),
        );
        let _ = self.bus.set(p.passed, Value::I32(self.passed));
        let _ = self.bus.set(p.failed, Value::I32(self.failed));
        let _ = self.bus.set(p.faults, Value::I32(self.faults));
    }

    /// Re-scan the build tree and publish what is flashable.
    ///
    /// Cheap — it stats a handful of paths — so it runs at startup and on every Rescan alongside
    /// the probe enumeration. A tree that has never been built is a first-class answer with the
    /// command that would fix it attached, not an empty list.
    fn rediscover(&mut self) {
        self.discovery = portal_swd::artefacts::discover();
        let p = &self.params;

        let _ = self
            .bus
            .set(p.image_count, Value::I32(self.discovery.found.len() as i32));

        for (slot, ids) in self.params.image_slots.iter().enumerate() {
            let found = self.discovery.found.get(slot);
            let (id, label, region, origin, detail, fits) = ids;
            let _ = self.bus.set_text(*id, found.map_or("", |a| a.id.as_str()));
            let _ = self
                .bus
                .set_text(*label, found.map_or("", |a| a.label.as_str()));
            let _ = self
                .bus
                .set_text(*region, found.map_or("", |a| a.region.as_str()));
            let _ = self.bus.set_text(
                *origin,
                found.map_or("", |a| match a.origin {
                    portal_swd::artefacts::Origin::Built => "built",
                    portal_swd::artefacts::Origin::Reference => "reference",
                }),
            );
            let _ = self.bus.set_text(
                *detail,
                &found.map_or(String::new(), |a| {
                    let size = format!("{:.1} kB", a.bytes as f64 / 1024.0);
                    if a.fits() {
                        size
                    } else {
                        format!("{size} — too large for its bank")
                    }
                }),
            );
            let _ = self
                .bus
                .set(*fits, Value::Bool(found.is_some_and(|a| a.fits())));
        }

        let hint = self
            .discovery
            .missing
            .iter()
            .map(|m| format!("{}: {}", m.label, m.hint))
            .collect::<Vec<_>>()
            .join(" · ");
        let _ = self.bus.set_text(p.image_hint, &hint);

        // Adopt sensible defaults the first time, so a rig with one obvious answer does not make
        // the operator click twice to say so. Anything already chosen is left alone.
        if schema::get_text(&self.bus, p.image_app_id).is_empty()
            && let Some(app) = self.discovery.application()
        {
            let _ = self.bus.set_text(p.image_app_id, &app.id);
        }
        if schema::get_text(&self.bus, p.image_boot_id).is_empty()
            && let Some(boot) = self.discovery.bootloader()
        {
            let _ = self.bus.set_text(p.image_boot_id, &boot.id);
        }

        self.reload_bundle();
    }

    /// Turn the current selection into something flashable, or say why not.
    fn reload_bundle(&mut self) {
        if self.simulated {
            // The synthetic bundle stands in for a build tree that may not exist; discovery has
            // nothing to do with it.
            return;
        }
        let selection = portal_swd::artefacts::Selection {
            bootloader: non_empty(schema::get_text(&self.bus, self.params.image_boot_id)),
            application: non_empty(schema::get_text(&self.bus, self.params.image_app_id)),
        };
        self.selection = selection.clone();

        match self.discovery.load(&selection) {
            Ok(bundle) => {
                self.bundle = Some(bundle);
                self.set_detail("");
            }
            Err(portal_swd::artefacts::LoadError::NothingSelected) => {
                self.bundle = None;
            }
            Err(err) => {
                self.bundle = None;
                self.set_detail(&err.to_string());
            }
        }
        self.publish_image();
    }

    fn publish_image(&self) {
        let p = &self.params;
        let _ = self.bus.set_text(p.image_scope, self.selection.scope());
        match &self.bundle {
            Some(bundle) => {
                let manifest = bundle.manifest();
                let _ = self.bus.set_text(p.image_name, "loaded");
                let _ = self.bus.set_text(
                    p.image_source,
                    match &manifest.provenance {
                        portal_swd::image::Provenance::Built { .. } => "built",
                        portal_swd::image::Provenance::Pulled { .. } => "pulled",
                        portal_swd::image::Provenance::Composed { .. } => "composed",
                        portal_swd::image::Provenance::Synthetic => "synthetic",
                    },
                );
                let build_id = match &manifest.provenance {
                    portal_swd::image::Provenance::Built {
                        git_commit,
                        git_dirty,
                        ..
                    } => format!("{git_commit}{}", if *git_dirty { "*" } else { "" }),
                    portal_swd::image::Provenance::Composed {
                        bootloader,
                        application,
                    } => format!("{bootloader} + {application}"),
                    _ => String::new(),
                };
                let _ = self.bus.set_text(p.image_build_id, &build_id);
                let _ = self
                    .bus
                    .set_text(p.image_boot_sha, &bundle.bootloader.sha256());
                let _ = self
                    .bus
                    .set_text(p.image_app_sha, &bundle.application.sha256());
            }
            None => {
                let _ = self.bus.set_text(p.image_name, "none");
                let _ = self.bus.set_text(p.image_source, "");
                let _ = self.bus.set_text(p.image_build_id, "");
                let _ = self.bus.set_text(p.image_boot_sha, "");
                let _ = self.bus.set_text(p.image_app_sha, "");
            }
        }
    }
}

fn non_empty(text: String) -> Option<String> {
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use av_gui_bus::SchemaBuilder;
    use portal_swd::SimRig;
    use std::sync::Mutex;

    /// A worker wired to a real sealed bus and a simulated target, with a clock we control.
    struct Harness {
        worker: Worker,
        bus: Arc<Bus>,
        params: Params,
        now: Millis,
        /// Held so `live_sessions()` is non-zero: a rig with no session is one nobody can hear.
        _session: av_gui_bus::Session,
    }

    impl Harness {
        fn new() -> Self {
            let mut builder = SchemaBuilder::new();
            crate::schema::declare(&mut builder, true).expect("schema");
            let bus = Arc::new(builder.seal());
            let params = Params::resolve(&bus).expect("params");
            let session = bus.open_session().expect("session");

            let sim = SimRig::new();
            let fixture = sim.fixture();
            let worker = Worker::new(
                Arc::clone(&bus),
                params.clone(),
                Box::new(sim),
                Some(crate::synthetic_bundle()),
                Arc::new(Mutex::new(None)),
                true,
                Some(fixture),
            );

            Self {
                worker,
                bus,
                params,
                now: 0,
                _session: session,
            }
        }

        fn tick(&mut self, ms: Millis, page_alive: bool) {
            self.now += ms;
            if page_alive {
                let _ = self
                    .bus
                    .set(self.params.heartbeat, Value::I64(self.now as i64));
            }
            self.worker.tick_at(self.now);
        }

        fn set_mode_auto(&self, auto: bool) {
            let _ = self
                .bus
                .set(self.params.mode_desired, Value::Enum(u32::from(auto)));
        }

        fn mode_is_auto(&self) -> bool {
            schema::get_enum(&self.bus, self.params.mode_desired) == 1
        }

        fn armed(&self) -> bool {
            self.worker.machine.armed()
        }

        fn arm(&mut self) {
            self.set_mode_auto(true);
            for _ in 0..20 {
                self.tick(80, true);
            }
            assert!(self.armed(), "the harness failed to arm");
        }

        fn seat_board(&self) {
            let _ = self.bus.set(
                self.params.sim_board_present.expect("sim"),
                Value::Bool(true),
            );
        }
    }

    #[test]
    fn a_live_page_keeps_auto_flash_armed() {
        let mut h = Harness::new();
        h.arm();
        for _ in 0..100 {
            h.tick(100, true);
        }
        assert!(h.armed());
        assert!(h.mode_is_auto());
    }

    /// The property the application contract requires: losing the UI cannot preserve arming.
    #[test]
    fn losing_the_page_drops_out_of_auto_flash() {
        let mut h = Harness::new();
        h.arm();
        for _ in 0..60 {
            h.tick(100, false);
        }
        assert!(!h.armed(), "the rig stayed armed with no page watching it");
    }

    /// The regression. Found by closing the tab and watching the rig read ARMED again.
    #[test]
    fn dropping_out_of_auto_flash_retracts_the_request_that_caused_it() {
        let mut h = Harness::new();
        h.arm();
        assert!(h.mode_is_auto());

        for _ in 0..60 {
            h.tick(100, false);
        }
        assert!(!h.armed());
        assert!(
            !h.mode_is_auto(),
            "/mode/desired survived the disarm, so the next tick re-arms and the dead-man does \
             nothing at all"
        );

        for _ in 0..60 {
            h.tick(100, true);
        }
        assert!(!h.armed(), "the rig re-armed itself without an operator");
    }

    #[test]
    fn manual_is_the_default_and_does_not_arm_anything() {
        let mut h = Harness::new();
        for _ in 0..40 {
            h.tick(100, true);
        }
        assert!(!h.armed());
        assert!(!h.mode_is_auto());
    }

    #[test]
    fn a_deliberate_switch_arms_again_after_a_drop_out() {
        let mut h = Harness::new();
        h.arm();
        for _ in 0..60 {
            h.tick(100, false);
        }
        assert!(!h.armed());
        h.arm();
        assert!(h.armed());
    }

    // ---------------------------------------------------------------- actions

    #[test]
    fn read_device_publishes_a_report_and_the_map_document() {
        let mut h = Harness::new();
        h.seat_board();
        h.tick(100, true);

        let _ = h.bus.set(h.params.act_read_device, Value::I64(1));
        h.tick(100, true);

        assert!(
            schema::get_bool(&h.bus, h.params.device_read),
            "a read should mark the device as read"
        );
        let guard = h.worker.device.lock().unwrap();
        let json = guard.as_ref().expect("the map document should exist");
        assert_eq!(json.occupancy.len(), crate::device_api::BUCKETS);
        assert_eq!(json.total_bytes, 128 * 1024);
        assert!(
            json.selected_occupancy.is_some(),
            "an image is selected, so the map should have a second lane to compare against"
        );
    }

    #[test]
    fn an_action_counter_fires_once_per_press() {
        let mut h = Harness::new();
        h.seat_board();
        h.tick(100, true);

        let _ = h.bus.set(h.params.act_read_device, Value::I64(7));
        h.tick(100, true);
        assert_eq!(h.worker.last_read, 7);

        // Ticking again without a new press must not re-read -- otherwise a reconnecting page
        // would re-trigger every action it had ever sent.
        let reads = h
            .worker
            .device
            .lock()
            .unwrap()
            .as_ref()
            .map(|d| d.read_at_ms);
        h.tick(100, true);
        let again = h
            .worker
            .device
            .lock()
            .unwrap()
            .as_ref()
            .map(|d| d.read_at_ms);
        assert_eq!(reads, again, "a tick without a press re-ran the action");
    }

    #[test]
    fn manual_flash_is_refused_while_auto_flash_is_armed() {
        let mut h = Harness::new();
        h.arm();
        let _ = h.bus.set(h.params.act_flash_now, Value::I64(1));
        h.tick(100, true);
        assert!(
            schema::get_text(&h.bus, h.params.detail).contains("auto-flash"),
            "the two paths must never run at once"
        );
    }

    #[test]
    fn manual_flash_is_refused_with_an_empty_fixture() {
        let mut h = Harness::new();
        h.tick(100, true);
        let _ = h.bus.set(h.params.act_flash_now, Value::I64(1));
        h.tick(100, true);
        assert!(
            schema::get_text(&h.bus, h.params.detail).contains("answering"),
            "flashing nothing should say so rather than failing obscurely"
        );
    }
}
