//! The one thread that owns the probe and runs the state machine.
//!
//! It is a plain OS thread, not a Tokio task, and deliberately so: it blocks for seconds at a
//! time inside a flash pass, and it must keep ticking whether or not anyone is looking at the
//! UI. The page is a view. Every decision is made here.
//!
//! The framework's bus has no write-notification callback — an application core polls it, the
//! way `example-console`'s does. That suits this worker exactly, because it already has a clock
//! of its own.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use av_gui_bus::{Bus, Value};
use portal_swd::{
    Action, Cue, ImageBundle, Input, Machine, Millis, Pass, Presence, Rig, RigError, RigErrorKind,
    Timing,
};

use crate::schema::{self, Params};

/// What the worker drives, and what it reports through.
pub struct Worker {
    bus: Arc<Bus>,
    params: Params,
    machine: Machine,
    rig: Box<dyn Rig>,
    bundle: Option<ImageBundle>,
    started: Instant,
    poll_period: Duration,
    cue_seq: i64,
    passed: i32,
    failed: i32,
    faults: i32,
    probe_open: bool,
    /// Last heartbeat value seen on the bus, to notice the page actually moving it.
    last_heartbeat_value: i64,
    /// Tracks the armed edge, so a self-disarm can retract the operator request that caused it.
    was_armed: bool,
    /// Present only under `--simulate`: the shared fixture the page's switch drives.
    fixture: Option<Arc<AtomicBool>>,
}

impl Worker {
    pub fn new(
        bus: Arc<Bus>,
        params: Params,
        rig: Box<dyn Rig>,
        bundle: Option<ImageBundle>,
        fixture: Option<Arc<AtomicBool>>,
    ) -> Self {
        let timing = Timing::default();
        Self {
            bus,
            params,
            machine: Machine::new(timing),
            rig,
            bundle,
            started: Instant::now(),
            poll_period: Duration::from_millis(timing.idle_poll_ms),
            cue_seq: 0,
            passed: 0,
            failed: 0,
            faults: 0,
            probe_open: false,
            last_heartbeat_value: 0,
            was_armed: false,
            fixture,
        }
    }

    fn now(&self) -> Millis {
        self.started.elapsed().as_millis() as Millis
    }

    pub fn run(mut self) {
        self.publish_image();
        loop {
            std::thread::sleep(self.poll_period);
            let now = self.now();
            self.tick_at(now);
        }
    }

    /// One pass of the loop, at a caller-supplied instant.
    ///
    /// Split out from `run` so the dead-man can be tested in milliseconds rather than by waiting
    /// three real seconds — and so it is tested at all, which the browser check that found the
    /// re-arming bug was not a substitute for.
    fn tick_at(&mut self, now: Millis) {
        // ---- the UI's own liveness, from two independent facts.
        //
        // `live_sessions` catches a closed tab; the heartbeat catches a page whose script has
        // wedged while its socket stays open. Either alone would miss one of them.
        let heartbeat = schema::get_i64(&self.bus, self.params.arm_heartbeat);
        let page_moving = heartbeat != self.last_heartbeat_value;
        self.last_heartbeat_value = heartbeat;
        if page_moving && self.bus.live_sessions() > 0 {
            self.step(now, Input::Heartbeat);
        }

        // ---- the operator's request
        let desired = schema::get_bool(&self.bus, self.params.arm_desired);
        if desired && !self.machine.armed() {
            self.step(now, Input::Arm);
        } else if !desired && self.machine.armed() {
            self.step(now, Input::Disarm);
        }

        // ---- keep the probe open, and say so
        if !self.probe_open {
            match self.rig.open() {
                Ok(info) => {
                    self.probe_open = true;
                    let _ = self.bus.set(self.params.probe_present, Value::Bool(true));
                    let _ = self.bus.set_text(self.params.probe_name, &info.name);
                    self.step(now, Input::ProbeRecovered);
                }
                Err(err) => {
                    let _ = self.bus.set(self.params.probe_present, Value::Bool(false));
                    let _ = self.bus.set_text(self.params.probe_name, "");
                    self.set_detail(&err.detail);
                    self.step(now, Input::ProbeError);
                }
            }
        }

        // ---- poll, unless a pass owns the probe
        if self.probe_open && !self.machine.pass_in_flight() {
            self.sync_simulation();
            match self.rig.poll() {
                Ok(Presence::Present) => self.step(now, Input::PollPresent),
                Ok(Presence::Absent) => self.step(now, Input::PollAbsent),
                Err(err) => self.on_rig_error(now, err),
            }
        }

        self.step(now, Input::Tick);
        // Last, deliberately. The machine can disarm itself part-way through this tick — the
        // dead-man trips inside whichever `step` happens to run after the deadline, usually a
        // poll — and the falling edge has to be observed *after* all of that. Checked before the
        // polls, it never sees the transition at all, and the next tick's arm decision reads a
        // `/arm/desired` nothing has cleared and arms straight back up.
        self.settle_arm_intent();
        self.publish_state();
    }

    /// Retract the operator's request when the rig disarms itself.
    ///
    /// Without this the dead-man does not work at all, and the failure is silent: the machine
    /// notices the page has gone and drops `armed`, then the very next tick reads `/arm/desired`
    /// — still `true`, because nothing cleared it — and arms straight back up. Measured, not
    /// theorised: closing the tab left the rig reading ARMED with nobody able to hear it.
    ///
    /// So a self-disarm retracts the request too, and coming back means a deliberate press. An
    /// operator who walked away does not return to an armed rig.
    fn settle_arm_intent(&mut self) {
        let armed = self.machine.armed();
        if self.was_armed && !armed {
            let _ = self.bus.set(self.params.arm_desired, Value::Bool(false));
        }
        self.was_armed = armed;
    }

    /// Mirror the page's fixture switch onto the simulated target, so the fixture an operator
    /// toggles and the fixture the poll sees are the same thing.
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
            let _ = self.bus.set(self.params.probe_present, Value::Bool(false));
            self.step(now, Input::ProbeError);
        } else {
            // Not the probe: the target went away, which is an ordinary operator event and the
            // debounce's business rather than a fault.
            self.step(now, Input::PollAbsent);
        }
    }

    /// Feed the machine and carry out whatever it asked for.
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

        // One-shot failure injection, so an operator can hear the fail tone and watch the
        // removal gate without having to sabotage a board. Cleared as it is consumed.
        if let Some(id) = self.params.sim_fail_next
            && schema::get_bool(&self.bus, id)
        {
            let _ = self.bus.set(id, Value::Bool(false));
            self.set_detail("simulated failure");
            self.failed += 1;
            self.faults += 1;
            // Same instant: the machine does not time passes, and a fabricated clock in a test
            // should not have real time leak into it here.
            self.step(now, Input::PassDone { pass, ok: false });
            return;
        }

        let outcome = match pass {
            Pass::Flash => {
                let mut progress = |step, done, total| {
                    let _ = (step, done, total);
                };
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

        // Re-entrant into `step`, which is fine: the machine is a value, not a lock.

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
        let _ = self
            .bus
            .set(p.arm_observed, Value::Bool(self.machine.armed()));
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

    fn publish_image(&self) {
        let p = &self.params;
        match &self.bundle {
            Some(bundle) => {
                let manifest = bundle.manifest();
                let _ = self.bus.set_text(p.image_name, "loaded");
                let _ = self.bus.set_text(
                    p.image_source,
                    match &manifest.provenance {
                        portal_swd::image::Provenance::Built { .. } => "built",
                        portal_swd::image::Provenance::Pulled { .. } => "pulled",
                        portal_swd::image::Provenance::Synthetic => "synthetic",
                    },
                );
                let build_id = match &manifest.provenance {
                    portal_swd::image::Provenance::Built {
                        git_commit,
                        git_dirty,
                        ..
                    } => format!("{git_commit}{}", if *git_dirty { "*" } else { "" }),
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
            }
        }
    }
}

/// A rig for a machine with no probe backend compiled in yet.
///
/// It reports the probe as gone rather than pretending, which puts the page into `probe-lost`
/// with the reason on screen. That is the honest state for a build that cannot talk to hardware,
/// and it exercises the same recovery path a real USB dropout would.
#[derive(Debug, Default)]
pub struct NoRig;

impl Rig for NoRig {
    fn open(&mut self) -> Result<portal_swd::ProbeInfo, RigError> {
        Err(RigError::new(
            RigErrorKind::ProbeGone,
            "no probe backend in this build; run with --simulate",
        ))
    }

    fn poll(&mut self) -> Result<Presence, RigError> {
        Err(RigError::new(RigErrorKind::ProbeGone, "no probe backend"))
    }

    fn flash(
        &mut self,
        _bundle: &ImageBundle,
        _progress: &mut portal_swd::rig::Progress<'_>,
    ) -> Result<portal_swd::FlashReport, RigError> {
        Err(RigError::new(RigErrorKind::ProbeGone, "no probe backend"))
    }

    fn run_check(
        &mut self,
        _spec: &portal_swd::RunCheckSpec,
    ) -> Result<portal_swd::RunCheckReport, RigError> {
        Err(RigError::new(RigErrorKind::ProbeGone, "no probe backend"))
    }

    fn close(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use av_gui_bus::SchemaBuilder;
    use portal_swd::SimRig;

    /// A worker wired to a real sealed bus and a simulated target, with a clock we control.
    struct Harness {
        worker: Worker,
        bus: Arc<Bus>,
        params: Params,
        now: Millis,
        /// Held so `live_sessions()` is non-zero: a rig with no session is a rig nobody can hear.
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
                params,
                Box::new(sim),
                Some(crate::synthetic_bundle()),
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

        /// Advance and tick, optionally delivering the heartbeat a live page would have sent.
        fn tick(&mut self, ms: Millis, page_alive: bool) {
            self.now += ms;
            if page_alive {
                let _ = self
                    .bus
                    .set(self.params.arm_heartbeat, Value::I64(self.now as i64));
            }
            self.worker.tick_at(self.now);
        }

        fn set_desired(&self, armed: bool) {
            let _ = self.bus.set(self.params.arm_desired, Value::Bool(armed));
        }

        fn desired(&self) -> bool {
            crate::schema::get_bool(&self.bus, self.params.arm_desired)
        }

        fn armed(&self) -> bool {
            self.worker.machine.armed()
        }

        fn arm(&mut self) {
            self.set_desired(true);
            for _ in 0..20 {
                self.tick(80, true);
            }
            assert!(self.armed(), "the harness failed to arm");
        }
    }

    #[test]
    fn a_live_page_keeps_the_rig_armed() {
        let mut h = Harness::new();
        h.arm();
        for _ in 0..100 {
            h.tick(100, true);
        }
        assert!(h.armed());
        assert!(h.desired());
    }

    /// The property the application contract requires: losing the UI cannot preserve arming.
    ///
    /// Sound lives in the browser, so an armed rig with no page is a rig flashing boards that
    /// nobody can hear pass or fail.
    #[test]
    fn losing_the_page_disarms_the_rig() {
        let mut h = Harness::new();
        h.arm();

        // The page stops answering. Nothing else changes.
        for _ in 0..60 {
            h.tick(100, false);
        }

        assert!(!h.armed(), "the rig stayed armed with no page watching it");
    }

    /// The regression. Found by closing the tab and watching the rig read ARMED again.
    ///
    /// The machine disarmed correctly; the worker then read `/arm/desired`, still `true` because
    /// nothing had cleared it, and armed straight back up on the next tick. The dead-man was
    /// therefore completely inert while every one of its own unit tests passed, because they
    /// tested the machine rather than the loop around it.
    #[test]
    fn a_self_disarm_retracts_the_request_that_caused_it() {
        let mut h = Harness::new();
        h.arm();
        assert!(h.desired());

        for _ in 0..60 {
            h.tick(100, false);
        }
        assert!(!h.armed());
        assert!(
            !h.desired(),
            "/arm/desired survived the disarm, so the next tick will re-arm and the dead-man \
             does nothing at all"
        );

        // And it stays down: no amount of further ticking brings it back without a fresh press.
        for _ in 0..60 {
            h.tick(100, true);
        }
        assert!(!h.armed(), "the rig re-armed itself without an operator");
    }

    #[test]
    fn a_deliberate_press_arms_again_after_a_self_disarm() {
        let mut h = Harness::new();
        h.arm();
        for _ in 0..60 {
            h.tick(100, false);
        }
        assert!(!h.armed());

        // The operator comes back and presses arm.
        h.arm();
        assert!(h.armed());
    }
}
