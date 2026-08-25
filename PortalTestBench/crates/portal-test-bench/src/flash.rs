//! Production SWD flashing integrated into the bench worker.
//!
//! Policy remains in `portal_swd::Machine`; this adapter owns one rig, the selected production
//! bundle and the small serialisable snapshot consumed by the page and HTTP API.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use portal_swd::artefacts::{Origin, Selection};
use portal_swd::{
    Action, BootFault, DeviceReport, DeviceSettings, Discovery, IdentityState, Input, Machine,
    Pass, Presence, Rig, RigError, RigErrorKind, Sequence, SettingsSource, Step, Timing,
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
            machine: Machine::with_sequence(Timing::default(), Sequence::FlashEveryInsertion),
            discovery,
            selection,
            snapshot: FlashSnapshot::default(),
            auto_enabled: false,
            next_poll_ms: 0,
            last_present: false,
            identity_read_after_ms: 0,
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
        let report = self
            .rig
            .write_persistent(serial, settings, false, &mut |step, done, total| {
                progress(
                    &step.to_string(),
                    if total == 0 {
                        0.0
                    } else {
                        done as f64 / total as f64
                    },
                );
            })
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
        let report = self.rig.flash(bundle, &mut |step: Step, done, total| {
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

    fn flash_provision_and_boot(
        &mut self,
        bundle: &portal_swd::ImageBundle,
        serial: u32,
        settings: DeviceSettings,
        allow_identity_override: bool,
        progress: &mut dyn FnMut(&str, f64),
    ) -> Result<String, RigError> {
        let report = {
            let state = &mut self.snapshot;
            self.rig.flash(bundle, &mut |step: Step, done, total| {
                state.step = step.to_string();
                state.progress = if total == 0 {
                    0.0
                } else {
                    done as f64 / total as f64
                };
                progress(&state.step, state.progress);
            })?
        };
        self.snapshot.step = "identity".into();
        progress("identity", 1.0);
        let persistent = {
            let state = &mut self.snapshot;
            self.rig.write_persistent(
                serial,
                settings,
                allow_identity_override,
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
            )?
        };
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
            return Err(RigError::new(
                RigErrorKind::BadBundle,
                "provisioning requires an application image",
            ));
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
