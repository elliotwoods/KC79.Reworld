//! Production SWD flashing integrated into the bench worker.
//!
//! Policy remains in `portal_swd::Machine`; this adapter owns one rig, the selected production
//! bundle and the small serialisable snapshot consumed by the page and HTTP API.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use portal_swd::artefacts::{Origin, Selection};
use portal_swd::{
    Action, BootFault, DeviceReport, DeviceSettings, Discovery, IdentityState, Input, Machine,
    Pass, Presence, Release, Rig, RigError, RigErrorKind, Sequence, SettingsSource, Step, Timing,
};

/// The production bootloader deliberately listens for an RS485 update for three seconds before
/// handing off. Boot verification must allow that interval, then prove the application remains
/// selected long enough to reject a watchdog reset loop.
const BOOT_HANDOFF_TIMEOUT: Duration = Duration::from_secs(5);
const BOOT_STABILITY_WINDOW: Duration = Duration::from_millis(600);
const TARGET_SETTLE_MS: u64 = 250;
const IDENTITY_RETRY_MS: u64 = 250;

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
    pub bootloader_sha256: String,
    pub application_sha256: String,
    pub identity_state: String,
    pub provision_serial: Option<u32>,
    pub operating_current_ma: u16,
    pub full_current_home_recovery: bool,
    pub axis_a_calibration: Option<portal_swd::OpticalCalibration>,
    pub axis_b_calibration: Option<portal_swd::OpticalCalibration>,
    pub settings_corrupt_records: u32,
    pub settings_source: String,
    pub option_bytes: String,
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
    pub boot_state: String,
    pub boot_detail: String,
    pub needs_replug: bool,
    /// How many times a simulated board has been restarted since the bench started, and `None`
    /// on hardware, where the count is what the operator watches rather than something the rig
    /// reports. Published because "how many times did it restart" is the question this pass was
    /// answering wrongly, and a number beats counting homing cycles across the room.
    pub simulated_starts: Option<u32>,
    pub mcu: Option<McuSnapshot>,
}

pub struct FieldUpdateTarget {
    pub application: Vec<u8>,
    pub expected_sha256: String,
    run_check: portal_swd::RunCheckSpec,
    bootloader_before: Vec<u8>,
    persistent_before: Vec<u8>,
}

pub struct FieldUpdateEvidence {
    pub readback_sha256: String,
    pub bootloader_unchanged: bool,
    pub persistent_unchanged: bool,
    pub application_booted: bool,
    pub detail: String,
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
    identity_read_after_ms: u64,
    fixture: Option<Arc<AtomicBool>>,
    /// The simulated rig's restart count, so a test can hold a pass to exactly one.
    starts: Option<Arc<AtomicU32>>,
    simulated: bool,
    probe_selector: String,
}

impl FlashController {
    pub fn new(simulated: bool) -> Self {
        Self::new_in(portal_swd::discover(), simulated)
    }

    /// The same, over a discovery that was resolved somewhere other than this repository.
    ///
    /// `portal_swd::discover()` walks the tree the binary was compiled in, which is right for the
    /// bench and impossible for a test: a pass cannot be driven without artefacts to load, and a
    /// test that depended on somebody having built the firmware first would pass or fail for
    /// reasons that have nothing to do with it. `artefacts::discover_in` already resolves a tree
    /// anywhere, so this simply lets the caller say which.
    pub fn new_in(discovery: Discovery, simulated: bool) -> Self {
        let selection = Selection {
            bootloader: discovery.bootloader().map(|a| a.id.clone()),
            application: discovery.application().map(|a| a.id.clone()),
        };
        let probe_selector = if simulated {
            "sim".to_string()
        } else {
            adopt_selector("", &portal_swd::list_probes()).unwrap_or_default()
        };
        type SimHandles = (Box<dyn Rig>, Option<Arc<AtomicBool>>, Option<Arc<AtomicU32>>);
        let (rig, fixture, starts): SimHandles = if simulated {
            let rig = portal_swd::SimRig::new();
            let (fixture, starts) = (rig.fixture(), rig.starts());
            (Box::new(rig), Some(fixture), Some(starts))
        } else {
            (
                Box::new(portal_swd::ProbeRsRig::new(
                    (!probe_selector.is_empty()).then(|| probe_selector.clone()),
                )),
                None,
                None,
            )
        };
        let mut this = Self {
            rig,
            machine: Machine::with_sequence(Timing::default(), Sequence::FlashEveryInsertion),
            discovery,
            selection,
            snapshot: FlashSnapshot::default(),
            auto_enabled: false,
            next_poll_ms: 0,
            last_present: false,
            identity_read_after_ms: 0,
            fixture,
            starts,
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

    fn refresh_start_count(&mut self) {
        self.snapshot.simulated_starts = self
            .starts
            .as_ref()
            .map(|starts| starts.load(Ordering::Relaxed));
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
            adopt_selector("", &portal_swd::list_probes()).unwrap_or_default()
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
        self.snapshot.mcu = None;
        self.last_present = false;
        self.identity_read_after_ms = 0;
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
                    "variant": a.variant,
                    "hardware": a.hardware,
                    "banner": a.banner,
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
        // Rescan the tree this controller was built over, rather than asking where the firmware
        // is all over again. For the bench the two are the same path; the difference is that a
        // controller pointed somewhere else by `new_in` stays pointed there instead of quietly
        // reverting to the repository on the first rescan.
        self.discovery = match self.discovery.root.as_deref() {
            Some(root) => portal_swd::artefacts::discover_in(root),
            None => portal_swd::discover(),
        };
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
            if let Some(adopted) = adopt_selector(&self.probe_selector, &probes) {
                self.probe_selector = adopted;
            }
            self.rig.close();
            self.rig = Box::new(portal_swd::ProbeRsRig::new(
                (!self.probe_selector.is_empty()).then(|| self.probe_selector.clone()),
            ));
        }
        self.snapshot.probe_connected = false;
        self.snapshot.target_present = false;
        self.snapshot.mcu = None;
        self.last_present = false;
        self.identity_read_after_ms = 0;
        self.open_probe();
    }

    pub fn select(&mut self, boot_id: String, app_id: String) {
        self.selection.bootloader = (!boot_id.is_empty()).then_some(boot_id);
        self.selection.application = (!app_id.is_empty()).then_some(app_id);
        self.refresh_selection_snapshot();
    }

    pub fn tick(&mut self, now_ms: u64, page_heartbeat: bool, auto_enabled: bool) -> Option<Pass> {
        self.refresh_start_count();
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
                    // Never carry the previous board's identity across an insertion. Give the
                    // newly attached target a short electrical settling window before doing the
                    // larger flash/identity read.
                    self.snapshot.mcu = None;
                    self.identity_read_after_ms = now_ms.saturating_add(TARGET_SETTLE_MS);
                    self.snapshot.detail = "MCU connected; waiting to read identity".into();
                    self.last_present = true;
                }
                if self.snapshot.mcu.is_none() {
                    if now_ms < self.identity_read_after_ms {
                        self.sync_machine();
                        return None;
                    }
                    if !self.read_device() {
                        self.identity_read_after_ms = now_ms.saturating_add(IDENTITY_RETRY_MS);
                        self.sync_machine();
                        return None;
                    }
                }
                // Presence only counts toward auto-flash debounce after the identity read has
                // succeeded. This guarantees provisioning sees the UID and existing serial.
                let actions = self.machine.step(now_ms, Input::PollPresent);
                self.begin_from(actions)
            }
            Ok(Presence::Absent) => {
                if self.last_present {
                    // The observing ARM interface belongs to the board that was just removed.
                    // Detach it now so the next insertion cannot inherit cached DP/AP state
                    // from a different MCU.
                    self.rig.close();
                    self.snapshot.probe_connected = false;
                }
                self.snapshot.target_present = false;
                self.snapshot.mcu = None;
                self.last_present = false;
                self.identity_read_after_ms = 0;
                let actions = self.machine.step(now_ms, Input::PollAbsent);
                self.begin_from(actions)
            }
            Err(error) => {
                self.snapshot.probe_connected = false;
                self.snapshot.target_present = false;
                self.snapshot.mcu = None;
                self.last_present = false;
                self.identity_read_after_ms = 0;
                self.snapshot.detail = error.to_string();
                let actions = self.machine.step(now_ms, Input::ProbeError);
                self.begin_from(actions)
            }
        }
    }

    pub fn manual_ready(&self) -> Result<(), String> {
        self.swd_ready()?;
        self.discovery
            .load(&self.selection)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    pub fn swd_ready(&self) -> Result<(), String> {
        if self.snapshot.busy {
            return Err("an SWD operation is already running".into());
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
        Ok(())
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
        if pass == Pass::Flash {
            self.snapshot.needs_replug = false;
            self.snapshot.boot_state = "checking".into();
            self.snapshot.boot_detail.clear();
        }
        self.snapshot.step = match pass {
            Pass::Flash => "attach".into(),
            Pass::RunCheck => "run-check".into(),
        };
        self.snapshot.phase = pass.to_string();

        let result = match pass {
            Pass::Flash => match self.discovery.load(&self.selection) {
                Ok(bundle) => self.flash_and_boot(&bundle, progress),
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
                if pass == Pass::Flash {
                    self.snapshot.boot_state = "not-running".into();
                    self.snapshot.boot_detail = error.to_string();
                }
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

    /// Program firmware and the durable board records as one provisioning operation. Firmware
    /// programming is bounded below the persistent pages; the identity write remains append-only
    /// unless the caller has recorded an explicit operator override.
    pub fn provision(
        &mut self,
        now_ms: u64,
        serial: u32,
        settings: DeviceSettings,
        allow_identity_override: bool,
        automatic: bool,
        progress: &mut dyn FnMut(&str, f64),
    ) -> bool {
        self.snapshot.busy = true;
        self.snapshot.progress = 0.0;
        self.snapshot.needs_replug = false;
        self.snapshot.boot_state = "checking".into();
        self.snapshot.boot_detail.clear();
        self.snapshot.step = "attach".into();
        self.snapshot.phase = Pass::Flash.to_string();

        let result = match self.discovery.load(&self.selection) {
            Ok(_bundle) if self.selection.bootloader.is_none() => Err(RigError::new(
                RigErrorKind::BadBundle,
                "provisioning requires the production bootloader image",
            )),
            Ok(_bundle)
                if self
                    .selection
                    .bootloader
                    .as_deref()
                    .and_then(|id| self.discovery.by_id(id))
                    .is_none_or(|artefact| artefact.origin != Origin::Built) =>
            {
                Err(RigError::new(
                    RigErrorKind::BadBundle,
                    "provisioning refuses the legacy reference bootloader; build PortalBootloader so the bounded updater is selected",
                ))
            }
            Ok(bundle) => self.flash_provision_and_boot(
                &bundle,
                serial,
                settings,
                allow_identity_override,
                progress,
            ),
            Err(error) => Err(RigError::new(RigErrorKind::BadBundle, error.to_string())),
        };
        self.finish_pass(now_ms, Pass::Flash, automatic, result)
    }

    pub fn selected_application_sha256(&self) -> Option<String> {
        self.discovery
            .load(&self.selection)
            .ok()
            .map(|bundle| bundle.application.sha256())
    }

    pub fn selected_bundle_evidence(&self) -> Option<(String, String)> {
        self.discovery
            .load(&self.selection)
            .ok()
            .map(|bundle| (bundle.sha256(), format!("{:?}", bundle.provenance)))
    }

    pub fn prepare_field_update(&mut self) -> Result<FieldUpdateTarget, String> {
        self.swd_ready()?;
        let bundle = self
            .discovery
            .load(&self.selection)
            .map_err(|error| error.to_string())?;
        if bundle.application.bytes.len() <= 65_536 {
            return Err(format!(
                "selected application is {} bytes; the bootloader check requires an image larger than 65536 bytes",
                bundle.application.bytes.len()
            ));
        }

        let image = self.rig.read_device().map_err(|error| error.to_string())?;
        let report = image.analyse();
        if !report.layout.supports_field_update() {
            return Err(format!(
                "device layout `{}` has no resident RS485 bootloader",
                report.layout.as_str()
            ));
        }
        let boot_end = portal_swd::addr::BOOTLOADER_BYTES as usize;
        let persist_at = (portal_swd::addr::PERSIST_BASE - portal_swd::addr::FLASH_BASE) as usize;
        if image.flash.len() < persist_at {
            return Err("SWD readback was shorter than the persistent partition".into());
        }
        self.snapshot.mcu = Some(mcu_snapshot(report));
        // Start this test from the resident bootloader, independent of whatever blocking
        // startup/motion routine the application happens to be in. The reset vector is in the
        // protected bootloader bank; the immediately queued RS485 announcements then keep its
        // receive window open for the full transfer.
        self.rig
            .reset_and_run()
            .map_err(|error| format!("could not reset into the resident bootloader: {error}"))?;
        Ok(FieldUpdateTarget {
            expected_sha256: bundle.application.sha256(),
            application: bundle.application.bytes.clone(),
            run_check: bundle.run_check.clone(),
            bootloader_before: image.flash[..boot_end].to_vec(),
            persistent_before: image.flash[persist_at..].to_vec(),
        })
    }

    pub fn verify_field_update(
        &mut self,
        target: &FieldUpdateTarget,
    ) -> Result<FieldUpdateEvidence, String> {
        let image = self.rig.read_device().map_err(|error| error.to_string())?;
        let app_at = (portal_swd::addr::APP_BASE - portal_swd::addr::FLASH_BASE) as usize;
        let persist_at = (portal_swd::addr::PERSIST_BASE - portal_swd::addr::FLASH_BASE) as usize;
        let app_end = app_at + target.application.len();
        if image.flash.len() < persist_at || app_end > persist_at {
            return Err("SWD readback cannot cover the selected application bank".into());
        }

        let bootloader_unchanged = image.flash[..app_at] == target.bootloader_before;
        let persistent_unchanged = image.flash[persist_at..] == target.persistent_before;
        let application_matches = image.flash[app_at..app_end] == target.application;
        let tail_erased = image.flash[app_end..persist_at]
            .iter()
            .all(|byte| *byte == 0xFF);
        let readback_sha256 = portal_swd::device::sha256_hex(&image.flash[app_at..app_end]);
        let report = image.analyse();
        self.snapshot.mcu = Some(mcu_snapshot(report));

        if !application_matches || readback_sha256 != target.expected_sha256 {
            let mismatch = image.flash[app_at..app_end]
                .iter()
                .zip(&target.application)
                .position(|(actual, expected)| actual != expected);
            let mismatch_detail = mismatch.map_or_else(
                || "no byte mismatch found".to_string(),
                |offset| {
                    format!(
                        "first mismatch at application +0x{offset:08X} (flash 0x{:08X}): expected 0x{:02X}, got 0x{:02X}",
                        portal_swd::addr::APP_BASE + offset as u32,
                        target.application[offset],
                        image.flash[app_at + offset]
                    )
                },
            );
            return Err(format!(
                "RS485 upload readback mismatch: expected {}, got {}; {mismatch_detail}",
                short_hash(&target.expected_sha256),
                short_hash(&readback_sha256)
            ));
        }
        if !tail_erased {
            return Err("application-bank bytes after the selected image were not erased".into());
        }
        if !bootloader_unchanged {
            return Err("the bootloader bank changed during the RS485 upload".into());
        }
        if !persistent_unchanged {
            return Err("the provisioning/settings pages changed during the RS485 upload".into());
        }

        self.observe_boot(target.run_check.vtor)
            .map_err(|error| error.to_string())?;
        Ok(FieldUpdateEvidence {
            readback_sha256: readback_sha256.clone(),
            bootloader_unchanged,
            persistent_unchanged,
            application_booted: true,
            detail: format!(
                "bootloader upload passed: {} bytes verified {}, bootloader and persistent pages unchanged, application stable",
                target.application.len(),
                short_hash(&readback_sha256)
            ),
        })
    }

    pub fn write_settings(
        &mut self,
        settings: DeviceSettings,
        progress: &mut dyn FnMut(&str, f64),
    ) -> Result<String, String> {
        self.swd_ready()?;
        let serial = self
            .snapshot
            .mcu
            .as_ref()
            .and_then(|mcu| mcu.provision_serial)
            .ok_or_else(|| "the board has no valid provisioning identity".to_string())?;
        // Restarting is the point here: the application reads its settings once, at boot, so a
        // write the board never restarts to pick up has changed nothing it can act on.
        let report = self
            .rig
            .write_persistent(
                serial,
                settings,
                false,
                Release::Run,
                &mut |step, done, total| {
                    progress(
                        &step.to_string(),
                        if total == 0 {
                            0.0
                        } else {
                            done as f64 / total as f64
                        },
                    );
                },
            )
            .map_err(|error| error.to_string())?;
        self.read_device();
        Ok(format!(
            "settings {} at {} mA; recovery {}",
            if report.settings_written {
                "written"
            } else {
                "unchanged"
            },
            report.settings.operating_current_ma,
            if report.settings.full_current_home_recovery {
                "enabled"
            } else {
                "disabled"
            },
        ))
    }

    pub fn verified_skip(&mut self, now_ms: u64, automatic: bool, detail: String) -> bool {
        self.snapshot.boot_state = "checking".into();
        let result = self
            .discovery
            .load(&self.selection)
            .map_err(|error| RigError::new(RigErrorKind::BadBundle, error.to_string()))
            .and_then(|bundle| self.observe_boot(bundle.run_check.vtor))
            .map(|boot| format!("{detail}; {boot}"));
        match result {
            Ok(detail) => {
                self.snapshot.last_outcome = format!("verified skip: {detail}");
                self.snapshot.detail = detail;
                self.snapshot.progress = 1.0;
                if automatic {
                    self.apply_machine(
                        now_ms,
                        Input::PassDone {
                            pass: Pass::Flash,
                            ok: true,
                        },
                    );
                } else {
                    self.snapshot.phase = "manual-complete".into();
                }
                true
            }
            Err(error) => {
                self.snapshot.last_outcome = format!("verified skip failed: {error}");
                self.snapshot.detail = error.to_string();
                if automatic {
                    self.apply_machine(
                        now_ms,
                        Input::PassDone {
                            pass: Pass::Flash,
                            ok: false,
                        },
                    );
                }
                false
            }
        }
    }

    /// Reset through SWD and prove that execution reached the selected application.
    pub fn reset_and_run(&mut self) -> Result<String, String> {
        self.swd_ready()?;
        let bundle = self
            .discovery
            .load(&self.selection)
            .map_err(|e| e.to_string())?;
        if bundle.run_check.vtor == 0 {
            return Err("the selected firmware has no application to boot".into());
        }
        self.snapshot.busy = true;
        self.snapshot.phase = "reset-run".into();
        self.snapshot.step = "reset-run".into();
        self.snapshot.boot_state = "checking".into();
        self.snapshot.boot_detail.clear();
        self.snapshot.needs_replug = false;

        let result = self
            .rig
            .reset_and_run()
            .and_then(|()| self.observe_boot(bundle.run_check.vtor));
        self.finish_boot_action("reset & run", result)
    }

    /// Observe boot state without altering the MCU.
    pub fn check_boot(&mut self) -> Result<String, String> {
        self.swd_ready()?;
        let bundle = self
            .discovery
            .load(&self.selection)
            .map_err(|e| e.to_string())?;
        if bundle.run_check.vtor == 0 {
            return Err("the selected firmware has no application to check".into());
        }
        self.snapshot.busy = true;
        self.snapshot.phase = "boot-check".into();
        self.snapshot.step = "boot-check".into();
        self.snapshot.boot_state = "checking".into();
        self.snapshot.boot_detail.clear();
        let result = self.observe_boot(bundle.run_check.vtor);
        self.finish_boot_action("boot check", result)
    }

    pub fn read_device(&mut self) -> bool {
        match self.rig.read_device() {
            Ok(image) => {
                self.snapshot.mcu = Some(mcu_snapshot(image.analyse()));
                true
            }
            Err(error) => {
                self.snapshot.mcu = None;
                self.snapshot.detail = format!("MCU connected; identity read retrying: {error}");
                // A target can answer DPIDR while the previous board's cached ARM interface is
                // no longer able to open its memory AP. Retry from a fresh probe/interface
                // rather than repeating the same failed session forever.
                self.rig.close();
                self.snapshot.probe_connected = false;
                false
            }
        }
    }

    /// Close a pass which the adapter could not prepare after the pure machine committed it.
    /// Without this acknowledgement the machine remains Busy forever and cannot recover.
    pub fn reject_pass(&mut self, now_ms: u64, pass: Pass, detail: &str) {
        self.snapshot.detail = detail.into();
        self.snapshot.last_outcome = format!("{pass} refused before start: {detail}");
        self.apply_machine(now_ms, Input::PassDone { pass, ok: false });
    }

    pub fn keep_progress_at_least(&mut self, fraction: f64) {
        self.snapshot.progress = self.snapshot.progress.max(fraction.clamp(0.0, 1.0));
    }

    fn flash_and_boot(
        &mut self,
        bundle: &portal_swd::ImageBundle,
        progress: &mut dyn FnMut(&str, f64),
    ) -> Result<String, portal_swd::RigError> {
        let state = &mut self.snapshot;
        // Nothing follows this write, so the pass ends here and the board starts here: one
        // restart, as with the provisioning path, just reached a shorter way.
        let report = self
            .rig
            .flash(bundle, Release::Run, &mut |step: Step, done, total| {
                state.step = step.to_string();
                state.progress = if total == 0 {
                    0.0
                } else {
                    done as f64 / total as f64
                };
                progress(&state.step, state.progress);
            })?;
        let verified = format!("verified {}", short_hash(&report.readback_sha256));

        // Bootloader-only programming has nothing to enter or observe.
        if bundle.run_check.vtor == 0 {
            self.snapshot.boot_state = "not-applicable".into();
            self.snapshot.boot_detail = "no application was selected".into();
            return Ok(verified);
        }

        self.snapshot.step = "boot-check".into();
        progress("boot-check", 1.0);
        match self.observe_boot(bundle.run_check.vtor) {
            Ok(detail) => Ok(format!("{verified}; {detail}")),
            Err(first) => {
                // A second explicit reset covers a debugger halt that survived the programming
                // session. It is safe because readback is already complete.
                self.snapshot.step = "reset-run".into();
                progress("reset-run", 1.0);
                if let Err(reset_error) = self.rig.reset_and_run() {
                    if report.option_bytes_programmed {
                        self.snapshot.boot_state = "replug-required".into();
                        self.snapshot.needs_replug = true;
                        self.snapshot.boot_detail = format!(
                            "option bytes changed; unplug and replug this virgin board, then check boot ({reset_error})"
                        );
                        return Ok(format!("{verified}; replug required before startup"));
                    }
                    return Err(reset_error);
                }
                match self.observe_boot(bundle.run_check.vtor) {
                    Ok(detail) => Ok(format!("{verified}; {detail} after reset retry")),
                    Err(second) if report.option_bytes_programmed => {
                        // The first option-byte reload on a factory-fresh part can require power
                        // to be removed. The flash contents are verified; do not tell the operator
                        // to reflash a good image, but do make the required replug impossible to
                        // mistake for a completed boot.
                        self.snapshot.boot_state = "replug-required".into();
                        self.snapshot.needs_replug = true;
                        self.snapshot.boot_detail = format!(
                            "option bytes changed; unplug and replug this virgin board, then check boot ({second})"
                        );
                        Ok(format!("{verified}; replug required before startup"))
                    }
                    Err(second) => Err(portal_swd::RigError::new(
                        portal_swd::RigErrorKind::NotRunning,
                        format!(
                            "boot check failed after reset retry: {second}; first check: {first}"
                        ),
                    )),
                }
            }
        }
    }

    /// Put a board back on its feet after a stage that released it halted.
    ///
    /// Between the first `Release::Halt` and the pass's own restart the core is stopped, so a
    /// failure in that window would otherwise hand the operator a board that is simply dead —
    /// no motion, no VCOM, nothing on the wire — with an error message about something else
    /// entirely. Best effort by definition; if the restart fails too, both failures are named,
    /// because "the flash failed" and "and it is still stopped" are different things to do next.
    fn restart_after_failure(&mut self, error: RigError) -> RigError {
        match self.rig.reset_and_run() {
            Ok(()) => error,
            Err(restart) => RigError::new(
                error.kind,
                format!("{error}; the board was also left halted: {restart}"),
            ),
        }
    }

    fn flash_provision_and_boot(
        &mut self,
        bundle: &portal_swd::ImageBundle,
        serial: u32,
        settings: DeviceSettings,
        allow_identity_override: bool,
        progress: &mut dyn FnMut(&str, f64),
    ) -> Result<String, RigError> {
        // Both writing stages release *halted*, so the application does not start between them.
        // The board comes up once, below, running firmware and durable records that are both
        // already in place -- rather than starting on the firmware, being stopped again for the
        // identity write, and starting a second time. On a module that homes its prisms at
        // startup, each of those starts is ten seconds of motion the operator has to watch.
        let report = {
            let state = &mut self.snapshot;
            self.rig
                .flash(bundle, Release::Halt, &mut |step: Step, done, total| {
                    state.step = step.to_string();
                    state.progress = if total == 0 {
                        0.0
                    } else {
                        done as f64 / total as f64
                    };
                    progress(&state.step, state.progress);
                })
        };
        // From here to the restart the core is stopped, so every exit has to go back through
        // `restart_after_failure` or the board is left dead in the fixture.
        let report = report.map_err(|error| self.restart_after_failure(error))?;

        self.snapshot.step = "identity".into();
        progress("identity", 1.0);
        let persistent = {
            let state = &mut self.snapshot;
            self.rig.write_persistent(
                serial,
                settings,
                allow_identity_override,
                Release::Halt,
                &mut |step: Step, done, total| {
                    state.step = step.to_string();
                    progress(
                        &state.step,
                        if total == 0 {
                            0.0
                        } else {
                            done as f64 / total as f64
                        },
                    );
                },
            )
        };
        let persistent = persistent.map_err(|error| self.restart_after_failure(error))?;
        let verified = format!(
            "verified firmware {}; identity {} serial {}; settings {} at {} mA",
            short_hash(&report.readback_sha256),
            if persistent.identity_written {
                "written"
            } else {
                "preserved"
            },
            persistent.serial,
            if persistent.settings_written {
                "written"
            } else {
                "unchanged"
            },
            persistent.settings.operating_current_ma,
        );

        if bundle.run_check.vtor == 0 {
            return Err(self.restart_after_failure(RigError::new(
                RigErrorKind::BadBundle,
                "provisioning requires an application image",
            )));
        }

        // The one restart of the pass, and the pass does not pass until it has happened.
        self.snapshot.step = "reset-run".into();
        progress("reset-run", 1.0);
        if let Err(error) = self.rig.reset_and_run() {
            self.snapshot.boot_state = "not-running".into();
            self.snapshot.boot_detail = error.to_string();
            return Err(error);
        }
        self.snapshot.step = "boot-check".into();
        progress("boot-check", 1.0);
        match self.observe_boot(bundle.run_check.vtor) {
            Ok(detail) => Ok(format!("{verified}; {detail}")),
            Err(first) => {
                if let Err(reset_error) = self.rig.reset_and_run() {
                    if report.option_bytes_programmed {
                        self.snapshot.boot_state = "replug-required".into();
                        self.snapshot.needs_replug = true;
                        self.snapshot.boot_detail = format!(
                            "option bytes changed; unplug and replug this board ({reset_error})"
                        );
                        return Ok(format!("{verified}; replug required before startup"));
                    }
                    return Err(reset_error);
                }
                match self.observe_boot(bundle.run_check.vtor) {
                    Ok(detail) => Ok(format!("{verified}; {detail} after reset retry")),
                    Err(second) if report.option_bytes_programmed => {
                        self.snapshot.boot_state = "replug-required".into();
                        self.snapshot.needs_replug = true;
                        self.snapshot.boot_detail = format!(
                            "option bytes changed; unplug and replug this board ({second})"
                        );
                        Ok(format!("{verified}; replug required before startup"))
                    }
                    Err(second) => Err(RigError::new(
                        RigErrorKind::NotRunning,
                        format!(
                            "boot check failed after reset retry: {second}; first check: {first}"
                        ),
                    )),
                }
            }
        }
    }

    fn finish_pass(
        &mut self,
        now_ms: u64,
        pass: Pass,
        automatic: bool,
        result: Result<String, RigError>,
    ) -> bool {
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
                self.snapshot.last_outcome = format!("{pass} passed: {detail}");
            }
            Err(error) => {
                self.snapshot.detail = error.to_string();
                self.snapshot.last_outcome = format!("{pass} failed: {error}");
                self.snapshot.boot_state = "not-running".into();
                self.snapshot.boot_detail = error.to_string();
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
        self.refresh_start_count();
        ok
    }

    fn observe_boot(&mut self, expected_vtor: u32) -> Result<String, RigError> {
        let started = Instant::now();
        loop {
            let report = self.rig.boot_check(expected_vtor)?;
            match report.verdict(expected_vtor) {
                Ok(()) => break,
                Err(BootFault::WrongVectorTable { .. } | BootFault::ResetDuringWindow)
                    if started.elapsed() < BOOT_HANDOFF_TIMEOUT =>
                {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(fault) => return Err(boot_error(fault, report.rcc_csr)),
            }
        }

        // A momentary visit to the application vector table is not enough. In particular, the
        // IWDG remains active across the bootloader jump and exposes a broken startup as a reset
        // loop. Re-observe after a stable window so that failure cannot earn a PASS.
        std::thread::sleep(BOOT_STABILITY_WINDOW);
        let stable = self.rig.boot_check(expected_vtor)?;
        stable
            .verdict(expected_vtor)
            .map_err(|fault| boot_error(fault, stable.rcc_csr))?;
        self.snapshot.boot_state = "running".into();
        self.snapshot.boot_detail = format!(
            "application stable at VTOR {:#010X} (RCC_CSR {:#010X})",
            stable.vtor, stable.rcc_csr
        );
        self.snapshot.needs_replug = false;
        Ok("application started and remained stable".into())
    }

    fn finish_boot_action(
        &mut self,
        label: &str,
        result: Result<String, portal_swd::RigError>,
    ) -> Result<String, String> {
        self.snapshot.busy = false;
        self.snapshot.progress = if result.is_ok() { 1.0 } else { 0.0 };
        match result {
            Ok(detail) => {
                self.snapshot.detail = detail.clone();
                self.snapshot.last_outcome = format!("{label} passed: {detail}");
                self.snapshot.phase = "manual-complete".into();
                Ok(detail)
            }
            Err(error) => {
                self.snapshot.boot_state = "not-running".into();
                self.snapshot.boot_detail = error.to_string();
                self.snapshot.detail = error.to_string();
                self.snapshot.last_outcome = format!("{label} failed: {error}");
                self.snapshot.phase = "manual-complete".into();
                Err(error.to_string())
            }
        }
    }

    fn open_probe(&mut self) {
        if !self.simulated && self.probe_selector.is_empty() {
            // The refusal below already had to enumerate, so adopting rides along for free -- and
            // this is the path that used to leave a bench running all day without knowing which
            // probe it was running on. A bench started before its fixture is plugged in resolves
            // no selector, `ProbeRsRig::new(None)` then opens "the first probe", and flashing
            // works: the SWD side self-heals and nothing says otherwise. But `/probe/selected`
            // stays empty, so the page cannot mark the row chosen, `ProbeInfo::serial` (which is
            // the selector) stays empty, and `paired_vcom_port` can never match -- so the
            // post-flash VCOM handover fails for the whole of its five-second deadline.
            //
            // Nothing open is disturbed by rebuilding the rig here: this runs from `new_in`, from
            // `select_probe` and `rescan` (which both close first), and from `tick` only while
            // `probe_connected` is already false.
            let attached = portal_swd::list_probes();
            if attached.len() > 1 {
                self.snapshot.probe_connected = false;
                self.snapshot.detail = "multiple ST-Links found; choose the fixture probe".into();
                return;
            }
            if let Some(adopted) = adopt_selector(&self.probe_selector, &attached) {
                self.probe_selector = adopted;
                // Rebuild the owner around the selector rather than leaving a `None` rig that
                // happens to open the same device: the selector is what the rig reports as its
                // identity, and both the VCOM pairing and the page's list need it.
                self.rig.close();
                self.rig = Box::new(portal_swd::ProbeRsRig::new(Some(
                    self.probe_selector.clone(),
                )));
            }
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

/// Which probe an unselected fixture should adopt.
///
/// Empty in, empty out unless there is exactly one probe attached. Guessing which ST-Link on a
/// bench is the fixture is exactly the kind of helpfulness that flashes the wrong board, and a
/// selection already made is never second-guessed.
///
/// Stated once, as a plain function, for two reasons: the constructor, the operator's picker, a
/// rescan and the reopen path all have to apply the *same* rule, and this way the rule can be
/// tested with no probe attached.
fn adopt_selector(current: &str, attached: &[portal_swd::ProbeDescriptor]) -> Option<String> {
    if !current.is_empty() {
        return None;
    }
    match attached {
        [only] => Some(only.id.clone()),
        _ => None,
    }
}

fn short_hash(hash: &str) -> &str {
    hash.get(..12).unwrap_or(hash)
}

fn boot_error(fault: BootFault, rcc_csr: u32) -> RigError {
    let reset_cause = if rcc_csr & (1 << 29) != 0 {
        "; RCC_CSR records an independent-watchdog reset"
    } else {
        ""
    };
    RigError::new(
        RigErrorKind::NotRunning,
        format!("{fault}; RCC_CSR={rcc_csr:#010X}{reset_cause}"),
    )
}

fn mcu_snapshot(report: DeviceReport) -> McuSnapshot {
    let firmware = report
        .application
        .banner
        .clone()
        .or_else(|| report.bootloader.banner.clone())
        .unwrap_or_default();
    let (identity_state, provision_serial) = match report.identity {
        IdentityState::Blank => ("blank".to_string(), None),
        IdentityState::Corrupt => ("corrupt".to_string(), None),
        IdentityState::ForeignUid { record } => ("foreign-uid".to_string(), Some(record.serial)),
        IdentityState::Valid { record } => ("existing-on-board".to_string(), Some(record.serial)),
    };
    let settings_source = match report.settings.source {
        SettingsSource::Defaults => "defaults",
        SettingsSource::FlashA | SettingsSource::FlashB => "flash",
    };
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
        bootloader_sha256: report.bootloader.sha256,
        application_sha256: report.application.sha256,
        identity_state,
        provision_serial,
        operating_current_ma: report.settings.record.settings.operating_current_ma,
        full_current_home_recovery: report.settings.record.settings.full_current_home_recovery,
        axis_a_calibration: report.settings.record.settings.axis_a_calibration,
        axis_b_calibration: report.settings.record.settings.axis_b_calibration,
        settings_corrupt_records: report.settings.corrupt_records,
        settings_source: settings_source.into(),
        option_bytes: format!("0x{:08X}", report.options.raw),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(id: &str) -> portal_swd::ProbeDescriptor {
        portal_swd::ProbeDescriptor {
            id: id.into(),
            name: "STLink V2-1".into(),
            serial: Some(id.rsplit(':').next().unwrap().into()),
            vendor_id: 0x0483,
            product_id: 0x374b,
            kind: "ST-LINK".into(),
        }
    }

    /// The hardware-free core of the fix: an unselected fixture adopts the only probe there is,
    /// and never picks between two.
    ///
    /// The bug this closes was invisible because the fixture recovered anyway. A bench started
    /// before its ST-Link was plugged in resolved no selector; `ProbeRsRig::new(None)` then opened
    /// "the first probe" and flashing worked all day — while the page's probe list sat empty
    /// beside a badge reading "connected", and the post-flash VCOM handover could never pair,
    /// because both need a selector and the selector was resolved once, at construction, and
    /// never again.
    #[test]
    fn an_unselected_fixture_adopts_the_only_probe_and_never_guesses_between_two() {
        let one = [descriptor("0483:374b:PROBE123")];
        let two = [
            descriptor("0483:374b:PROBE123"),
            descriptor("0483:374b:OTHER456"),
        ];

        assert_eq!(adopt_selector("", &[]), None, "nothing attached, nothing to adopt");
        assert_eq!(
            adopt_selector("", &one),
            Some("0483:374b:PROBE123".to_string()),
            "one probe and no choice made: adopt it"
        );
        assert_eq!(
            adopt_selector("", &two),
            None,
            "two probes and no choice made: refuse to guess which one is the fixture"
        );
        assert_eq!(
            adopt_selector("0483:374b:PROBE123", &two),
            None,
            "a choice already made is never second-guessed"
        );
    }

    /// A firmware tree with a *built* bootloader beside a built application, in the layout
    /// `discover_in` expects. Built rather than the committed reference, because provisioning
    /// refuses the reference image and would never reach a pass.
    fn firmware_tree(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("portal-bench-{name}"));
        let _ = std::fs::remove_dir_all(&dir);

        let mut application = vec![0u8; 60_000];
        application[0..4].copy_from_slice(&0x2000_9000u32.to_le_bytes());
        application[4..8]
            .copy_from_slice(&(portal_swd::addr::APP_BASE + 0x241).to_le_bytes());
        let app = dir.join("PortalFW/.pio/build/application_bank_optical");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("firmware.bin"), application).unwrap();

        let boot = dir.join("PortalBootloader/.pio/build/bootloader");
        std::fs::create_dir_all(&boot).unwrap();
        std::fs::write(boot.join("firmware.bin"), vec![0xA5; 19_568]).unwrap();
        dir
    }

    /// The pass restarts the board once, and the call order is what decides that.
    ///
    /// `portal-swd` holds the rule that a halted release does not start the application; this
    /// holds the thing that can silently undo it — the sequence in `flash_provision_and_boot`.
    /// Release either stage to run instead of halted and every assertion below still passes
    /// except the count, which is exactly how this went unnoticed: an extra restart leaves a
    /// board that is running the right image, so nothing downstream looks wrong. The operator saw
    /// it because the module homes its prisms on startup and did it three times.
    #[test]
    fn a_provisioning_pass_restarts_the_board_exactly_once() {
        let root = firmware_tree("restarts-once");
        let mut controller =
            FlashController::new_in(portal_swd::artefacts::discover_in(&root), true);
        assert_eq!(controller.snapshot().scope, "full", "both regions selected");

        controller.set_sim_present(true);
        let _ = controller.tick(0, false, false);
        let _ = controller.tick(500, false, false);
        assert!(controller.snapshot().mcu.is_some(), "identity was read");
        assert_eq!(
            controller.snapshot().simulated_starts,
            Some(0),
            "nothing has restarted the board yet"
        );

        let passed = controller.provision(
            1_000,
            7,
            DeviceSettings::default(),
            false,
            false,
            &mut |_, _| {},
        );
        assert!(passed, "{}", controller.snapshot().last_outcome);
        assert_eq!(
            controller.snapshot().boot_state,
            "running",
            "and it came back up: {}",
            controller.snapshot().boot_detail
        );
        assert_eq!(
            controller.snapshot().simulated_starts,
            Some(1),
            "firmware and durable records are written with the core halted, so the board starts \
             once, at the end, running everything that was written"
        );
    }

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
        assert!(controller.snapshot().mcu.is_none());
        let _ = controller.tick(500, false, false);
        assert_eq!(
            controller.snapshot().mcu.as_ref().map(|m| m.part.as_str()),
            Some("STM32G070RBT6")
        );

        controller.set_sim_present(false);
        let _ = controller.tick(1_000, false, false);
        assert!(!controller.snapshot().target_present);
        assert!(controller.snapshot().mcu.is_none());
        assert!(!controller.snapshot().probe_connected);

        let _ = controller.tick(1_500, false, false);
        assert!(controller.snapshot().probe_connected);
        // The reopen above runs `open_probe`, which is now also where an empty selector is
        // resolved against what is attached. Simulation must never reach that: a machine with a
        // real ST-Link plugged in while the bench is simulating must not have it adopted.
        assert_eq!(controller.probe_selector(), "sim");
    }

    #[test]
    fn auto_flash_waits_for_identity_and_a_rejected_pass_can_disarm_cleanly() {
        let mut controller = FlashController::new(true);

        // Arming requires a real empty-fixture interval before accepting an insertion.
        for now in (0..=560).step_by(80) {
            assert_eq!(controller.tick(now, now == 0, true), None);
        }

        controller.set_sim_present(true);
        for now in [640, 720, 800, 880, 960, 1_040] {
            assert_eq!(controller.tick(now, false, true), None);
        }
        assert!(controller.snapshot().mcu.is_some());
        assert_eq!(controller.tick(1_120, false, true), Some(Pass::Flash));

        controller.reject_pass(1_120, Pass::Flash, "preparation failed");
        assert_eq!(controller.snapshot().phase, "await-removal");
        assert!(controller.snapshot().armed);

        let _ = controller.tick(1_200, false, false);
        assert_eq!(controller.snapshot().phase, "disarmed");
        assert!(!controller.snapshot().armed);
    }
}
