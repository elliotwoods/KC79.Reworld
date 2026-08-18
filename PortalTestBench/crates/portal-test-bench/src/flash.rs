//! Production SWD flashing integrated into the bench worker.
//!
//! Policy remains in `portal_swd::Machine`; this adapter owns one rig, the selected production
//! bundle and the small serialisable snapshot consumed by the page and HTTP API.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use portal_swd::artefacts::Selection;
use portal_swd::{
    Action, DeviceReport, Discovery, Input, Machine, Pass, Presence, Rig, Step, Timing,
};

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct McuSnapshot {
    pub part: String,
    pub uid: String,
    pub idcode: String,
    pub dev_id: String,
    pub flash_kb: u16,
    pub layout: String,
    pub rdp: String,
    pub firmware: String,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct FlashSnapshot {
    pub probe_connected: bool,
    pub target_present: bool,
    pub probe_name: String,
    pub probe_serial: String,
    pub probe_firmware: String,
    pub speed_khz: u32,
    pub armed: bool,
    pub busy: bool,
    pub phase: String,
    pub step: String,
    pub progress: f64,
    pub detail: String,
    pub last_outcome: String,
    pub boot_id: String,
    pub app_id: String,
    pub scope: String,
    pub mcu: Option<McuSnapshot>,
}

pub struct FlashController {
    rig: Box<dyn Rig>,
    machine: Machine,
    discovery: Discovery,
    selection: Selection,
    snapshot: FlashSnapshot,
    auto_enabled: bool,
    next_poll_ms: u64,
    last_present: bool,
    fixture: Option<Arc<AtomicBool>>,
    simulated: bool,
    probe_selector: String,
}

impl FlashController {
    pub fn new(simulated: bool) -> Self {
        let discovery = portal_swd::discover();
        let selection = Selection {
            bootloader: discovery.bootloader().map(|a| a.id.clone()),
            application: discovery.application().map(|a| a.id.clone()),
        };
        let probe_selector = if simulated {
            "sim".to_string()
        } else {
            let probes = portal_swd::list_probes();
            if probes.len() == 1 {
                probes[0].id.clone()
            } else {
                String::new()
            }
        };
        let (rig, fixture): (Box<dyn Rig>, Option<Arc<AtomicBool>>) = if simulated {
            let rig = portal_swd::SimRig::new();
            let fixture = rig.fixture();
            (Box::new(rig), Some(fixture))
        } else {
            (
                Box::new(portal_swd::ProbeRsRig::new(
                    (!probe_selector.is_empty()).then(|| probe_selector.clone()),
                )),
                None,
            )
        };
        let mut this = Self {
            rig,
            machine: Machine::new(Timing::default()),
            discovery,
            selection,
            snapshot: FlashSnapshot::default(),
            auto_enabled: false,
            next_poll_ms: 0,
            last_present: false,
            fixture,
            simulated,
            probe_selector,
        };
        this.refresh_selection_snapshot();
        this.open_probe();
        this
    }

    pub fn snapshot(&self) -> &FlashSnapshot {
        &self.snapshot
    }

    pub fn probe_selector(&self) -> &str {
        &self.probe_selector
    }

    /// Selecting a row never guesses. Rebuild the probe owner around probe-rs's own selector,
    /// leaving the rig closed when several probes are attached and no row is selected.
    pub fn select_probe(&mut self, selector: String) {
        let selector = if self.simulated {
            "sim".to_string()
        } else if selector.is_empty() {
            let probes = portal_swd::list_probes();
            if probes.len() == 1 {
                probes[0].id.clone()
            } else {
                String::new()
            }
        } else {
            selector
        };
        if selector == self.probe_selector {
            return;
        }
        self.rig.close();
        self.probe_selector = selector;
        self.snapshot.probe_connected = false;
        self.snapshot.target_present = false;
        self.snapshot.probe_name.clear();
        self.snapshot.probe_serial.clear();
        self.snapshot.probe_firmware.clear();
        self.last_present = false;
        if !self.simulated {
            self.rig = Box::new(portal_swd::ProbeRsRig::new(
                (!self.probe_selector.is_empty()).then(|| self.probe_selector.clone()),
            ));
        }
        self.open_probe();
    }

    pub fn set_sim_present(&self, present: bool) {
        if let Some(fixture) = &self.fixture {
            fixture.store(present, Ordering::Relaxed);
        }
    }

    pub fn artefacts_json(&self) -> serde_json::Value {
        let found = self
            .discovery
            .found
            .iter()
            .map(|a| {
                serde_json::json!({
                    "id": a.id,
                    "label": a.label,
                    "region": format!("{:?}", a.region).to_lowercase(),
                    "origin": format!("{:?}", a.origin).to_lowercase(),
                    "bytes": a.bytes,
                    "modified": a.modified,
                    "fits": a.fits(),
                    "selected": self.selection.bootloader.as_deref() == Some(&a.id)
                        || self.selection.application.as_deref() == Some(&a.id),
                })
            })
            .collect::<Vec<_>>();
        let missing = self
            .discovery
            .missing
            .iter()
            .map(|m| serde_json::json!({ "label": m.label, "path": m.path, "hint": m.hint }))
            .collect::<Vec<_>>();
        serde_json::json!({
            "root": self.discovery.root,
            "selection": {
                "bootloader": self.selection.bootloader,
                "application": self.selection.application,
                "scope": self.selection.scope(),
            },
            "found": found,
            "missing": missing,
        })
    }

    pub fn rescan(&mut self) {
        self.discovery = portal_swd::discover();
        if self
            .selection
            .bootloader
            .as_deref()
            .is_none_or(|id| self.discovery.by_id(id).is_none())
        {
            self.selection.bootloader = self.discovery.bootloader().map(|a| a.id.clone());
        }
        if self
            .selection
            .application
            .as_deref()
            .is_none_or(|id| self.discovery.by_id(id).is_none())
        {
            self.selection.application = self.discovery.application().map(|a| a.id.clone());
        }
        self.refresh_selection_snapshot();
        if !self.simulated {
            let probes = portal_swd::list_probes();
            if !self.probe_selector.is_empty()
                && !probes.iter().any(|probe| probe.id == self.probe_selector)
            {
                self.probe_selector.clear();
            }
            if self.probe_selector.is_empty() && probes.len() == 1 {
                self.probe_selector = probes[0].id.clone();
            }
            self.rig.close();
            self.rig = Box::new(portal_swd::ProbeRsRig::new(
                (!self.probe_selector.is_empty()).then(|| self.probe_selector.clone()),
            ));
        }
        self.snapshot.probe_connected = false;
        self.snapshot.target_present = false;
        self.open_probe();
    }

    pub fn select(&mut self, boot_id: String, app_id: String) {
        self.selection.bootloader = (!boot_id.is_empty()).then_some(boot_id);
        self.selection.application = (!app_id.is_empty()).then_some(app_id);
        self.refresh_selection_snapshot();
    }

    pub fn tick(&mut self, now_ms: u64, page_heartbeat: bool, auto_enabled: bool) -> Option<Pass> {
        if page_heartbeat {
            self.apply_machine(now_ms, Input::Heartbeat);
        }
        if auto_enabled != self.auto_enabled {
            self.auto_enabled = auto_enabled;
            self.apply_machine(
                now_ms,
                if auto_enabled {
                    Input::Arm
                } else {
                    Input::Disarm
                },
            );
        }
        self.apply_machine(now_ms, Input::Tick);

        if now_ms < self.next_poll_ms || self.snapshot.busy {
            self.sync_machine();
            return None;
        }
        self.next_poll_ms = now_ms + if self.machine.armed() { 80 } else { 500 };
        if !self.snapshot.probe_connected {
            self.open_probe();
        }
        if !self.snapshot.probe_connected {
            self.sync_machine();
            return None;
        }

        match self.rig.poll() {
            Ok(Presence::Present) => {
                self.snapshot.target_present = true;
                if !self.last_present {
                    self.read_device();
                }
                self.last_present = true;
                let actions = self.machine.step(now_ms, Input::PollPresent);
                self.begin_from(actions)
            }
            Ok(Presence::Absent) => {
                self.snapshot.target_present = false;
                self.last_present = false;
                let actions = self.machine.step(now_ms, Input::PollAbsent);
                self.begin_from(actions)
            }
            Err(error) => {
                self.snapshot.probe_connected = false;
                self.snapshot.target_present = false;
                self.snapshot.detail = error.to_string();
                let actions = self.machine.step(now_ms, Input::ProbeError);
                self.begin_from(actions)
            }
        }
    }

    pub fn manual_ready(&self) -> Result<(), String> {
        if self.snapshot.busy {
            return Err("a flash pass is already running".into());
        }
        if self.machine.armed() {
            return Err("auto-flash is armed".into());
        }
        if !self.snapshot.probe_connected {
            return Err("no ST-Link is connected".into());
        }
        if !self.snapshot.target_present {
            return Err("no MCU is answering".into());
        }
        self.discovery
            .load(&self.selection)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    pub fn execute(
        &mut self,
        now_ms: u64,
        pass: Pass,
        automatic: bool,
        progress: &mut dyn FnMut(&str, f64),
    ) -> bool {
        self.snapshot.busy = true;
        self.snapshot.progress = 0.0;
        self.snapshot.step = match pass {
            Pass::Flash => "attach".into(),
            Pass::RunCheck => "run-check".into(),
        };
        self.snapshot.phase = pass.to_string();

        let result = match pass {
            Pass::Flash => match self.discovery.load(&self.selection) {
                Ok(bundle) => {
                    let state = &mut self.snapshot;
                    self.rig
                        .flash(&bundle, &mut |step: Step, done, total| {
                            state.step = step.to_string();
                            state.progress = if total == 0 {
                                0.0
                            } else {
                                done as f64 / total as f64
                            };
                            progress(&state.step, state.progress);
                        })
                        .map(|report| format!("verified {}", short_hash(&report.readback_sha256)))
                }
                Err(error) => Err(portal_swd::RigError::new(
                    portal_swd::RigErrorKind::BadBundle,
                    error.to_string(),
                )),
            },
            Pass::RunCheck => match self.discovery.load(&self.selection) {
                Ok(bundle) => self.rig.run_check(&bundle.run_check).and_then(|report| {
                    report
                        .verdict(&bundle.run_check)
                        .map(|()| "application is running".to_string())
                        .map_err(|fault| {
                            portal_swd::RigError::new(
                                portal_swd::RigErrorKind::NotRunning,
                                fault.to_string(),
                            )
                        })
                }),
                Err(error) => Err(portal_swd::RigError::new(
                    portal_swd::RigErrorKind::BadBundle,
                    error.to_string(),
                )),
            },
        };

        self.snapshot.busy = false;
        self.snapshot.progress = if result.is_ok() {
            1.0
        } else {
            self.snapshot.progress
        };
        let ok = result.is_ok();
        match result {
            Ok(detail) => {
                self.snapshot.detail = detail.clone();
                self.snapshot.last_outcome = format!("{} passed: {detail}", pass);
            }
            Err(error) => {
                self.snapshot.detail = error.to_string();
                self.snapshot.last_outcome = format!("{} failed: {error}", pass);
            }
        }
        if automatic {
            self.apply_machine(now_ms, Input::PassDone { pass, ok });
        } else {
            self.snapshot.phase = "manual-complete".into();
        }
        if ok {
            self.read_device();
        }
        ok
    }

    pub fn read_device(&mut self) {
        match self.rig.read_device() {
            Ok(image) => {
                self.snapshot.mcu = Some(mcu_snapshot(image.analyse()));
            }
            Err(error) => self.snapshot.detail = error.to_string(),
        }
    }

    fn open_probe(&mut self) {
        if !self.simulated && self.probe_selector.is_empty() && portal_swd::list_probes().len() > 1
        {
            self.snapshot.probe_connected = false;
            self.snapshot.detail = "multiple ST-Links found; choose the fixture probe".into();
            return;
        }
        match self.rig.open() {
            Ok(info) => {
                self.snapshot.probe_connected = true;
                self.snapshot.probe_name = info.name;
                self.snapshot.probe_serial = info.serial.unwrap_or_default();
                self.snapshot.probe_firmware = info.firmware.unwrap_or_default();
                self.snapshot.speed_khz = info.speed_khz;
                self.snapshot.detail.clear();
                self.apply_machine(0, Input::ProbeRecovered);
            }
            Err(error) => {
                self.snapshot.probe_connected = false;
                self.snapshot.detail = error.to_string();
            }
        }
    }

    fn refresh_selection_snapshot(&mut self) {
        self.snapshot.boot_id = self.selection.bootloader.clone().unwrap_or_default();
        self.snapshot.app_id = self.selection.application.clone().unwrap_or_default();
        self.snapshot.scope = self.selection.scope().into();
    }

    fn apply_machine(&mut self, now_ms: u64, input: Input) {
        let actions = self.machine.step(now_ms, input);
        let _ = self.begin_from(actions);
        self.sync_machine();
    }

    fn begin_from(&mut self, actions: Vec<Action>) -> Option<Pass> {
        let mut pass = None;
        for action in actions {
            match action {
                Action::BeginPass(next) => pass = Some(next),
                Action::Sound(cue) => self.snapshot.detail = format!("{:?}", cue).to_lowercase(),
                // `tick` converts this duration to an absolute deadline. Remembering the
                // duration here is unnecessary and would mix relative and absolute times.
                Action::SetPollPeriod(_) => {}
            }
        }
        self.sync_machine();
        pass
    }

    fn sync_machine(&mut self) {
        self.snapshot.armed = self.machine.armed();
        if !self.snapshot.busy {
            self.snapshot.phase = self.machine.phase().to_string();
        }
    }
}

fn short_hash(hash: &str) -> &str {
    hash.get(..12).unwrap_or(hash)
}

fn mcu_snapshot(report: DeviceReport) -> McuSnapshot {
    let firmware = report
        .application
        .banner
        .clone()
        .or_else(|| report.bootloader.banner.clone())
        .unwrap_or_default();
    McuSnapshot {
        part: "STM32G070RBT6".into(),
        uid: report.uid,
        idcode: report
            .idcode
            .map(|v| format!("0x{v:08X}"))
            .unwrap_or_default(),
        dev_id: report
            .dev_id
            .map(|v| format!("0x{v:03X}"))
            .unwrap_or_default(),
        flash_kb: report.flash_kb,
        layout: report.layout.as_str().into(),
        rdp: report.options.rdp_level().to_string(),
        firmware,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simulated_fixture_presence_populates_the_same_probe_and_mcu_snapshot() {
        let mut controller = FlashController::new(true);
        assert!(controller.snapshot().probe_connected);
        assert!(!controller.snapshot().target_present);
        assert_eq!(controller.probe_selector(), "sim");
        controller.select_probe("some physical selector".into());
        assert_eq!(controller.probe_selector(), "sim");

        controller.set_sim_present(true);
        let _ = controller.tick(0, false, false);
        assert!(controller.snapshot().target_present);
        assert_eq!(
            controller.snapshot().mcu.as_ref().map(|m| m.part.as_str()),
            Some("STM32G070RBT6")
        );
    }
}
