//! The real hardware backend: a [`Rig`] backed by probe-rs and an ST-Link.
//!
//! # The connection has three states, and only one of them is invasive
//!
//! ```text
//! Closed ──open──▶ Idle(Probe) ──attach_to_unspecified──▶ Observing(ArmDebugInterface)
//!                       ▲                                          │
//!                       └──────────────── close() ◀────────────────┘
//! ```
//!
//! `Observing` is where the poll and the whole of [`read_device`](ProbeRsRig::read_device) live.
//! It writes **nothing** to the target: no halt, no reset, no `Session`. That last part matters
//! more than it looks — probe-rs's STM32G0 attach sequence read-modify-writes `RCC_APBENR1` and
//! writes `DBGMCU_CR`, which is a real race against an application that might be touching
//! `RCC_APBENR1` itself. A `Session` is created only for programming, and dropped immediately
//! after.
//!
//! # `attach_to_unspecified` first, always
//!
//! `Probe::try_into_arm_debug_interface` returns `NotAttached` unless the probe has already been
//! attached, and nothing in its signature says so.

use std::time::Duration;

use probe_rs::architecture::arm::{
    ArmDebugInterface, ArmError, FullyQualifiedApAddress, dp::DpAddress,
    sequences::DefaultArmSequence,
};
use probe_rs::flashing::{DownloadOptions, FlashProgress, ProgressEvent, ProgressOperation};
use probe_rs::probe::{DebugProbeSelector, Probe, WireProtocol, list::Lister};
use probe_rs::{CoreStatus, MemoryInterface, Permissions, Session};

use crate::device::DeviceImage;
use crate::image::{ImageBundle, RunCheckSpec};
use crate::rig::{
    BootReport, FlashReport, Presence, ProbeInfo, Progress, Rig, RigError, RigErrorKind,
    RunCheckReport, Step,
};
use crate::{addr, bits, program};

/// The chip name in probe-rs's built-in registry.
///
/// Borrowed from [`Manifest::TARGET`] rather than written again. The same string used to be a
/// literal in both places with nothing comparing them, so a part change would have been applied to
/// the attach and silently not to the manifest every image records itself with.
///
/// It lives there and not here for a second reason: this module is behind
/// `#[cfg(feature = "probe")]`, and a `--simulate` build with the probe backend compiled out still
/// has to be able to say which part it is pretending to be.
pub const TARGET: &str = crate::image::Manifest::TARGET;

/// A probe the operator could choose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeDescriptor {
    /// `"vid:pid:serial"` in probe-rs's own selector syntax, so it round-trips through
    /// `DebugProbeSelector::from_str` without this crate inventing a format.
    pub id: String,
    pub name: String,
    pub serial: Option<String>,
    pub vendor_id: u16,
    pub product_id: u16,
    pub kind: String,
}

/// Every probe currently attached.
///
/// Cheap — it enumerates USB descriptors and opens nothing, so the page can call it as often as
/// the operator presses Rescan.
pub fn list_probes() -> Vec<ProbeDescriptor> {
    Lister::new()
        .list_all()
        .into_iter()
        .map(|info| ProbeDescriptor {
            id: match &info.serial_number {
                Some(serial) => format!("{:04x}:{:04x}:{serial}", info.vendor_id, info.product_id),
                None => format!("{:04x}:{:04x}", info.vendor_id, info.product_id),
            },
            name: info.identifier.clone(),
            serial: info.serial_number.clone(),
            vendor_id: info.vendor_id,
            product_id: info.product_id,
            kind: info.probe_type(),
        })
        .collect()
}

enum Link {
    Closed,
    Idle(Probe),
    Observing(Box<dyn ArmDebugInterface>),
}

impl Link {
    fn take(&mut self) -> Link {
        std::mem::replace(self, Link::Closed)
    }
}

pub struct ProbeRsRig {
    /// Which probe to use. `None` means "the first one", which is right for a single-station rig
    /// and wrong the moment there are two, so the page always writes an explicit id once it has
    /// enumerated.
    selector: Option<String>,
    link: Link,
    speed_khz: u32,
}

impl std::fmt::Debug for ProbeRsRig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProbeRsRig")
            .field("selector", &self.selector)
            .field("speed_khz", &self.speed_khz)
            .finish_non_exhaustive()
    }
}

impl ProbeRsRig {
    pub fn new(selector: Option<String>) -> Self {
        Self {
            selector,
            link: Link::Closed,
            speed_khz: 1_800,
        }
    }

    /// Change which probe is used. Drops any existing connection, because it is no longer the
    /// connection that was asked for.
    pub fn select(&mut self, selector: Option<String>) {
        if self.selector != selector {
            self.selector = selector;
            self.close();
        }
    }

    pub fn selector(&self) -> Option<&str> {
        self.selector.as_deref()
    }

    /// Move to `Observing`, opening and attaching as far as needed.
    fn observe(&mut self) -> Result<&mut Box<dyn ArmDebugInterface>, RigError> {
        loop {
            match self.link.take() {
                Link::Observing(iface) => {
                    self.link = Link::Observing(iface);
                    let Link::Observing(iface) = &mut self.link else {
                        unreachable!("just stored");
                    };
                    return Ok(iface);
                }
                Link::Idle(mut probe) => {
                    // Required, and invisible in the signature of the call below.
                    if let Err(err) = probe.attach_to_unspecified() {
                        return Err(probe_gone("attach", err));
                    }
                    match probe.try_into_arm_debug_interface(DefaultArmSequence::create()) {
                        Ok(iface) => self.link = Link::Observing(iface),
                        Err((probe, err)) => {
                            self.link = Link::Idle(probe);
                            return Err(RigError::new(
                                RigErrorKind::ProbeGone,
                                format!("could not open the ARM interface: {err}"),
                            ));
                        }
                    }
                }
                Link::Closed => {
                    let probe = self.open_probe()?;
                    self.link = Link::Idle(probe);
                }
            }
        }
    }

    fn open_probe(&mut self) -> Result<Probe, RigError> {
        let lister = Lister::new();
        let mut probe = match &self.selector {
            Some(text) => {
                let selector: DebugProbeSelector = text.parse().map_err(|err| {
                    RigError::new(
                        RigErrorKind::ProbeGone,
                        format!("{text:?} is not a probe selector: {err}"),
                    )
                })?;
                lister
                    .open(selector)
                    .map_err(|err| probe_gone("open the selected probe", err))?
            }
            None => {
                let all = lister.list_all();
                let Some(info) = all.first() else {
                    return Err(RigError::new(
                        RigErrorKind::ProbeGone,
                        "no debug probes are attached",
                    ));
                };
                info.open()
                    .map_err(|err| probe_gone("open the probe", err))?
            }
        };

        probe
            .select_protocol(WireProtocol::Swd)
            .map_err(|err| probe_gone("select SWD", err))?;
        // The probe answers with what it actually applied, which is not always what was asked.
        self.speed_khz = probe.set_speed(self.speed_khz).unwrap_or(self.speed_khz);
        Ok(probe)
    }

    /// Read everything worth capturing, without halting or resetting.
    pub fn read_device(&mut self) -> Result<DeviceImage, RigError> {
        let iface = self.observe()?;
        iface
            .select_debug_port(DpAddress::Default)
            .map_err(|err| target_error("select the debug port", err))?;

        let ap = FullyQualifiedApAddress::v1_with_default_dp(0);
        let mut mem = iface
            .memory_interface(&ap)
            .map_err(|err| target_error("open the memory interface", err))?;

        let read32 = |mem: &mut dyn MemoryInterface<ArmError>, at: u32| {
            mem.read_word_32(u64::from(at))
                .map_err(|err| target_error("read a register", err))
        };

        let mut uid = [0u32; 3];
        mem.read_32(u64::from(addr::UID_BASE), &mut uid)
            .map_err(|err| target_error("read the UID", err))?;
        let flash_kb = mem
            .read_word_16(u64::from(addr::FLASHSIZE_BASE))
            .map_err(|err| target_error("read the flash size", err))?;
        let optr = read32(&mut *mem, addr::FLASH_OPTR)?;
        let rcc_csr = read32(&mut *mem, addr::RCC_CSR)?;
        // Readable in practice even with RCC_APBENR1.DBGEN clear, but not something to depend on:
        // a part that does gate it should still produce a usable report.
        let idcode = mem.read_word_32(u64::from(addr::DBGMCU_IDCODE)).ok();

        let mut flash = vec![0u8; (addr::FLASH_END - addr::FLASH_BASE) as usize];
        mem.read(u64::from(addr::FLASH_BASE), &mut flash)
            .map_err(|err| target_error("read flash", err))?;

        Ok(DeviceImage {
            flash,
            optr,
            idcode,
            uid,
            flash_kb,
            rcc_csr,
        })
    }

    pub fn speed_khz(&self) -> u32 {
        self.speed_khz
    }

    /// Open a fresh probe and connect with NRST held.
    ///
    /// Under reset, not `attach`, for one reason: the board in the fixture is running unknown
    /// firmware. It may be reconfiguring SWD pins, spinning in a watchdog loop, or sitting in
    /// `WFI`. Halting a running target and erasing it is the sequence that produces half-written
    /// boards; holding reset while the debug port comes up does not depend on the application
    /// cooperating at all.
    ///
    /// Erase-all permission is granted here because a pass is a chip erase by design. It is
    /// scoped to this session and nothing else in the crate constructs one.
    fn attach_under_reset(&mut self) -> Result<Session, RigError> {
        let probe = self.open_probe()?;
        probe
            .attach_under_reset(TARGET, Permissions::new().allow_erase_all())
            .map_err(|err| {
                // Which layer failed decides whether the operator reseats a board or looks at the
                // equipment, so it is worth getting right rather than reporting one flat error.
                let kind = match &err {
                    probe_rs::Error::Probe(_) => RigErrorKind::ProbeGone,
                    _ => RigErrorKind::ContactLost,
                };
                RigError::new(kind, format!("could not attach under reset: {err}"))
            })
    }

    /// Open a regular debug session when NRST is not routed through the fixture.
    ///
    /// This is only used by the explicit reset/recovery action. Programming still requires the
    /// safer attach-under-reset path because it may encounter arbitrary target firmware.
    fn attach_normally(&mut self) -> Result<Session, RigError> {
        let probe = self.open_probe()?;
        probe.attach(TARGET, Permissions::new()).map_err(|err| {
            let kind = match &err {
                probe_rs::Error::Probe(_) => RigErrorKind::ProbeGone,
                _ => RigErrorKind::ContactLost,
            };
            RigError::new(kind, format!("could not attach normally: {err}"))
        })
    }
}

/// What is worth reading off a halted part before anything is written to it.
struct Survey {
    idcode: u32,
    dev_id: u32,
    optr: u32,
    rcc_csr: u32,
}

/// Halt, freeze the watchdog, and read the identity registers.
///
/// `attach_under_reset` leaves the core halted, so the halt here is a belt-and-braces check
/// rather than the mechanism: if the core is *running* at this point then reset did not take, and
/// erasing a running target with a live watchdog is precisely the thing to refuse.
fn survey(session: &mut Session) -> Result<Survey, RigError> {
    let mut core = session.core(0).map_err(session_error)?;
    if !core.core_halted().map_err(session_error)? {
        core.halt(HALT_TIMEOUT).map_err(|err| {
            RigError::new(
                RigErrorKind::ResetIneffective,
                format!("the core did not halt after connect-under-reset: {err}"),
            )
        })?;
    }

    // Before the identity reads, so a watchdog cannot land between them and the erase.
    {
        let mut port = MemPort(&mut core);
        program::freeze_watchdog(&mut port, true).map_err(|fault| {
            RigError::new(
                RigErrorKind::ContactLost,
                format!("could not freeze the watchdog: {fault}"),
            )
        })?;
    }

    let read = |core: &mut probe_rs::Core<'_>, at: u32, what: &str| {
        core.read_word_32(u64::from(at)).map_err(|err| {
            RigError::new(
                RigErrorKind::ContactLost,
                format!("could not read {what}: {err}"),
            )
        })
    };

    // DBGMCU is clocked by now, so unlike the non-invasive poll this can rely on IDCODE.
    let idcode = read(&mut core, addr::DBGMCU_IDCODE, "DBGMCU_IDCODE")?;
    let optr = read(&mut core, addr::FLASH_OPTR, "FLASH_OPTR")?;
    let rcc_csr = read(&mut core, addr::RCC_CSR, "RCC_CSR")?;

    Ok(Survey {
        idcode,
        dev_id: idcode & 0xFFF,
        optr,
        rcc_csr,
    })
}

/// The same, for the re-attach after an option-byte reload.
///
/// Separate only so the intent reads: at this point the part has already been surveyed once and
/// accepted, and what is being established is whether the new option bytes came back.
fn survey_after_reload(session: &mut Session) -> Result<Survey, RigError> {
    survey(session).map_err(|err| {
        // A part that will not come back after `OBL_LAUNCH` is an option-byte failure, whatever
        // the layer underneath called it -- and it is the failure the operator most needs named,
        // because it is the one that can leave a board unusable.
        RigError::new(
            RigErrorKind::OptionBytes,
            format!("the part did not come back after the option-byte reload: {err}"),
        )
    })
}

/// Reset a session that was attached under reset and make the release explicit.
///
/// `Core::reset` is documented to continue execution, but a debugger halt can survive target-
/// specific reset sequences. Checking the observed state and issuing `run` only when the core is
/// actually halted avoids both failure modes: leaving a flashed MCU stopped, and stepping a core
/// that was already running.
fn reset_session_and_run(session: &mut Session) -> Result<(), RigError> {
    let mut core = session.core(0).map_err(session_error)?;
    {
        let mut port = MemPort(&mut core);
        // The reset clears DBGMCU anyway. Do not turn a successful flash into a failure because
        // this best-effort cleanup write was lost immediately before that reset.
        let _ = program::freeze_watchdog(&mut port, false);
    }
    core.reset().map_err(|err| {
        RigError::new(
            RigErrorKind::Program,
            format!("could not reset the programmed MCU: {err}"),
        )
    })?;
    std::thread::sleep(Duration::from_millis(25));
    match core.status().map_err(session_error)? {
        status if status.is_halted() => core.run().map_err(|err| {
            RigError::new(
                RigErrorKind::Program,
                format!("the MCU reset halted and could not be resumed: {err}"),
            )
        }),
        CoreStatus::LockedUp => Err(RigError::new(
            RigErrorKind::NotRunning,
            "the MCU locked up immediately after reset",
        )),
        CoreStatus::Running | CoreStatus::Sleeping | CoreStatus::Unknown => Ok(()),
        CoreStatus::Halted(_) => unreachable!("handled by is_halted"),
    }
}

/// Adapts a probe-rs memory interface to the narrow port [`program`] is written against.
struct MemPort<'a>(&'a mut dyn MemoryInterface<probe_rs::Error>);

impl program::Mem for MemPort<'_> {
    fn read32(&mut self, at: u32) -> Result<u32, String> {
        self.0
            .read_word_32(u64::from(at))
            .map_err(|err| err.to_string())
    }

    fn write32(&mut self, at: u32, value: u32) -> Result<(), String> {
        self.0
            .write_word_32(u64::from(at), value)
            .map_err(|err| err.to_string())?;
        // These are control registers, not RAM. A write still sitting in the probe's buffer has
        // not happened, and the next thing this code does is nearly always read back the bit it
        // just set to find out whether it did.
        self.0.flush().map_err(|err| err.to_string())
    }
}

/// Bridge probe-rs's flash progress onto the crate's own [`Step`] callback.
///
/// probe-rs reports each page as a delta; the operator page wants a running total against a
/// known denominator. `total` is the fallback denominator for the case where probe-rs does not
/// announce one, so a progress bar never divides by zero.
fn flash_progress<'a>(sink: &'a mut Progress<'_>, total: u64) -> FlashProgress<'a> {
    let mut erase_total = total;
    let mut program_total = total;
    let mut erased = 0u64;
    let mut written = 0u64;

    FlashProgress::new(move |event| match event {
        ProgressEvent::AddProgressBar {
            operation,
            total: Some(announced),
        } => match operation {
            ProgressOperation::Erase => erase_total = announced,
            ProgressOperation::Program => program_total = announced,
            _ => {}
        },
        ProgressEvent::Progress {
            operation, size, ..
        } => match operation {
            ProgressOperation::Erase => {
                erased = (erased + size).min(erase_total);
                sink(Step::Erase, erased, erase_total);
            }
            ProgressOperation::Program => {
                written = (written + size).min(program_total);
                sink(Step::Program, written, program_total);
            }
            _ => {}
        },
        // The deltas do not always reach the announced total exactly; the page should still land
        // on a full bar rather than on 99%.
        ProgressEvent::Finished(ProgressOperation::Erase) => {
            sink(Step::Erase, erase_total, erase_total)
        }
        ProgressEvent::Finished(ProgressOperation::Program) => {
            sink(Step::Program, program_total, program_total)
        }
        _ => {}
    })
}

fn session_error(err: probe_rs::Error) -> RigError {
    let kind = match &err {
        probe_rs::Error::Probe(_) => RigErrorKind::ProbeGone,
        _ => RigErrorKind::ContactLost,
    };
    RigError::new(kind, err.to_string())
}

fn option_error(fault: program::FlashFault) -> RigError {
    let kind = match fault {
        // The target stopped answering, which is a lifted board rather than an option-byte
        // problem -- and it matters, because the removal gate handles one and not the other.
        program::FlashFault::Bus(_) => RigErrorKind::ContactLost,
        _ => RigErrorKind::OptionBytes,
    };
    RigError::new(kind, fault.to_string())
}

impl Rig for ProbeRsRig {
    fn open(&mut self) -> Result<ProbeInfo, RigError> {
        // Opening is enough to learn the identity; attaching waits until something needs the bus.
        let probe = match self.link.take() {
            Link::Closed => self.open_probe()?,
            other => {
                self.link = other;
                let name = match &self.link {
                    Link::Observing(_) | Link::Idle(_) => "ST-Link".to_owned(),
                    Link::Closed => unreachable!(),
                };
                return Ok(ProbeInfo {
                    name,
                    serial: self.selector.clone(),
                    // probe-rs 0.32 does not expose the ST-Link firmware version, but it refuses
                    // to open anything below V2J26 -- so an open probe is a supported one.
                    firmware: None,
                    speed_khz: self.speed_khz,
                });
            }
        };
        let info = ProbeInfo {
            name: probe.get_name(),
            serial: self.selector.clone(),
            firmware: None,
            speed_khz: self.speed_khz,
        };
        self.link = Link::Idle(probe);
        Ok(info)
    }

    fn poll(&mut self) -> Result<Presence, RigError> {
        let iface = self.observe()?;
        // A line reset plus DPIDR: ~1 ms measured, and it touches no target memory at all.
        match iface.select_debug_port(DpAddress::Default) {
            Ok(()) => Ok(Presence::Present),
            Err(err) => {
                // An empty fixture and a lost contact look identical here, which is exactly what
                // the debounce and the removal gate exist for. Only a probe-layer failure means
                // the equipment is gone.
                if is_probe_failure(&err) {
                    self.close();
                    Err(probe_gone("poll", err))
                } else {
                    Ok(Presence::Absent)
                }
            }
        }
    }

    fn read_device(&mut self) -> Result<DeviceImage, RigError> {
        ProbeRsRig::read_device(self)
    }

    fn flash(
        &mut self,
        bundle: &ImageBundle,
        progress: &mut Progress<'_>,
    ) -> Result<FlashReport, RigError> {
        // Before the probe is touched at all: a bundle that cannot be flashed correctly should
        // never get as far as erasing a board.
        let faults = bundle.validate();
        if !faults.is_empty() {
            return Err(RigError::new(
                RigErrorKind::BadBundle,
                faults
                    .iter()
                    .map(|f| f.to_string())
                    .collect::<Vec<_>>()
                    .join("; "),
            ));
        }

        // A `Session` needs the raw probe, and `Observing` is holding it.
        self.close();

        progress(Step::Attach, 0, 1);
        let mut session = self.attach_under_reset()?;
        let mut survey = survey(&mut session)?;
        progress(Step::Attach, 1, 1);

        if survey.dev_id != 0 && survey.dev_id != bits::DEV_ID_STM32G07X {
            return Err(RigError::new(
                RigErrorKind::WrongTarget,
                format!(
                    "the part reports DEV_ID {:#05X}; this bundle is for {:#05X} ({TARGET})",
                    survey.dev_id,
                    bits::DEV_ID_STM32G07X
                ),
            ));
        }
        if survey.optr & bits::OPTR_RDP_MASK != bits::OPTR_RDP_LEVEL0 {
            // Recovering costs a mass erase with RDP regression, which is a deliberate,
            // operator-confirmed act and never something a routine pass performs on its own.
            return Err(RigError::new(
                RigErrorKind::ReadoutProtected,
                format!(
                    "FLASH_OPTR reads {:#010X}, so RDP is not level 0",
                    survey.optr
                ),
            ));
        }

        let optr_before = survey.optr;
        let mut option_bytes_programmed = false;
        if bundle.option_bytes.needs_programming(optr_before) {
            progress(Step::OptionBytes, 0, 1);
            let wanted = bundle.option_bytes.desired(optr_before);
            {
                let mut core = session.core(0).map_err(session_error)?;
                let mut port = MemPort(&mut core);
                program::program_option_bytes(&mut port, wanted).map_err(option_error)?;
                // From here the part is about to reset itself, so nothing may be inferred from
                // this session again.
                program::launch_option_bytes(&mut port);
            }
            drop(session);
            session = self.attach_under_reset()?;
            survey = survey_after_reload(&mut session)?;
            if survey.optr != wanted {
                return Err(option_error(program::FlashFault::NotTaken {
                    wanted,
                    found: survey.optr,
                }));
            }
            option_bytes_programmed = true;
            progress(Step::OptionBytes, 1, 1);
        }

        // Erase and program. Only the regions that have bytes are staged -- writing 0xFF over the
        // rest would be 128 kB on the wire to reach a state the chip erase already produced.
        let mut loader = session.target().flash_loader();
        for region in [&bundle.bootloader, &bundle.application] {
            if region.bytes.is_empty() {
                continue;
            }
            loader
                .add_data(u64::from(region.load_address), &region.bytes)
                .map_err(|err| {
                    RigError::new(
                        RigErrorKind::Program,
                        format!("could not stage the {} region: {err}", region.name.as_str()),
                    )
                })?;
        }

        let expected = bundle.expected_flash_image();
        let total = expected.len() as u64;
        {
            // From `image::strategy`, not from literals here. The settings page publishes those
            // same constants, so what it says about erasing and verifying cannot drift from what
            // this actually does -- which it would the moment the two were written down twice.
            // The reasoning for each value is on the constant.
            let mut options = DownloadOptions::default();
            options.keep_unwritten_bytes = crate::image::strategy::KEEP_UNWRITTEN;
            options.do_chip_erase = crate::image::strategy::CHIP_ERASE;
            options.verify = crate::image::strategy::PROBE_RS_VERIFY;
            options.progress = flash_progress(progress, total);
            loader.commit(&mut session, options).map_err(|err| {
                RigError::new(RigErrorKind::Program, format!("programming failed: {err}"))
            })?;
        }

        // Readback, against the device rather than against what was sent to it.
        progress(Step::Readback, 0, total);
        let mut actual = vec![0u8; expected.len()];
        {
            let mut core = session.core(0).map_err(session_error)?;
            core.read(u64::from(addr::FLASH_BASE), &mut actual)
                .map_err(|err| {
                    RigError::new(
                        RigErrorKind::Verify,
                        format!("could not read the device back: {err}"),
                    )
                })?;
        }
        if let Some(at) = actual.iter().zip(expected.iter()).position(|(a, b)| a != b) {
            return Err(RigError::new(
                RigErrorKind::Verify,
                format!(
                    "readback differs at {:#010X}: expected {:#04X}, got {:#04X}",
                    addr::FLASH_BASE as usize + at,
                    expected[at],
                    actual[at]
                ),
            ));
        }
        progress(Step::Readback, total, total);
        let readback_sha256 = crate::device::sha256_hex(&actual);

        // Let the watchdog run again before the application does, explicitly release any halt
        // that survived the reset sequence, then let go.
        progress(Step::ResetRun, 0, 1);
        let reset = reset_session_and_run(&mut session);
        drop(session);
        self.link = Link::Closed;
        reset?;
        progress(Step::ResetRun, 1, 1);

        Ok(FlashReport {
            idcode: survey.idcode,
            optr_before,
            optr_after: survey.optr,
            option_bytes_programmed,
            rcc_csr: survey.rcc_csr,
            readback_sha256,
        })
    }

    fn reset_and_run(&mut self) -> Result<(), RigError> {
        self.close();
        let mut session = match self.attach_under_reset() {
            Ok(session) => session,
            Err(under_reset) => {
                self.close();
                self.attach_normally().map_err(|normal| {
                    RigError::new(
                        normal.kind,
                        format!(
                            "MCU reset could not attach under reset ({under_reset}) or normally ({normal})"
                        ),
                    )
                })?
            }
        };
        let reset = reset_session_and_run(&mut session);
        drop(session);
        self.link = Link::Closed;
        reset
    }

    fn boot_check(&mut self, _expected_vtor: u32) -> Result<BootReport, RigError> {
        // Like run_check, this must not create a Session: observing boot must not perturb it.
        let iface = self.observe()?;
        iface
            .select_debug_port(DpAddress::Default)
            .map_err(|err| target_error("select the debug port", err))?;
        let ap = FullyQualifiedApAddress::v1_with_default_dp(0);
        let mut mem = iface
            .memory_interface(&ap)
            .map_err(|err| target_error("open the memory interface", err))?;
        let read32 = |mem: &mut dyn MemoryInterface<ArmError>, at: u32| {
            mem.read_word_32(u64::from(at))
                .map_err(|err| target_error("read boot state", err))
        };

        let vtor = read32(&mut *mem, addr::SCB_VTOR)?;
        // S_RESET_ST is sticky and cleared by the first read. If it is present in the second,
        // the MCU reset again during this observation window.
        let dhcsr_first = read32(&mut *mem, addr::DHCSR)?;
        std::thread::sleep(Duration::from_millis(75));
        let dhcsr_second = read32(&mut *mem, addr::DHCSR)?;
        let rcc_csr = read32(&mut *mem, addr::RCC_CSR)?;
        Ok(BootReport {
            vtor,
            dhcsr_first,
            dhcsr_second,
            rcc_csr,
        })
    }

    fn run_check(&mut self, spec: &RunCheckSpec) -> Result<RunCheckReport, RigError> {
        if spec.liveness_address == 0 {
            return Err(RigError::new(
                RigErrorKind::NotRunning,
                "this bundle has no liveness address, so a run-check could only prove the \
                 bootloader jumped somewhere -- not that anything is running",
            ));
        }

        // No `Session`, deliberately: probe-rs's STM32G0 attach sequence read-modify-writes
        // `RCC_APBENR1` and writes `DBGMCU_CR`, which is a real race against the application this
        // is supposed to be observing.
        let iface = self.observe()?;
        iface
            .select_debug_port(DpAddress::Default)
            .map_err(|err| target_error("select the debug port", err))?;
        let ap = FullyQualifiedApAddress::v1_with_default_dp(0);
        let mut mem = iface
            .memory_interface(&ap)
            .map_err(|err| target_error("open the memory interface", err))?;

        let read32 = |mem: &mut dyn MemoryInterface<ArmError>, at: u32| {
            mem.read_word_32(u64::from(at))
                .map_err(|err| target_error("read a register during the run-check", err))
        };

        let vtor = read32(&mut *mem, addr::SCB_VTOR)?;
        // First, because `S_RESET_ST` is sticky and clears on read: this read establishes the
        // window, and the second one below is the one that means something.
        let dhcsr_first = read32(&mut *mem, addr::DHCSR)?;
        let liveness_first = read32(&mut *mem, spec.liveness_address)?;

        std::thread::sleep(Duration::from_millis(spec.window_ms));

        let dhcsr_second = read32(&mut *mem, addr::DHCSR)?;
        let liveness_second = read32(&mut *mem, spec.liveness_address)?;
        let rcc_csr = read32(&mut *mem, addr::RCC_CSR)?;

        Ok(RunCheckReport {
            vtor,
            dhcsr_first,
            dhcsr_second,
            liveness_first,
            liveness_second,
            rcc_csr,
        })
    }

    fn close(&mut self) {
        match self.link.take() {
            Link::Observing(iface) => {
                let mut probe = iface.close();
                let _ = probe.detach();
            }
            Link::Idle(mut probe) => {
                let _ = probe.detach();
            }
            Link::Closed => {}
        }
    }
}

impl Drop for ProbeRsRig {
    fn drop(&mut self) {
        self.close();
    }
}

/// Did the *probe* fail, or did the target simply not answer?
///
/// The distinction drives everything downstream: a lost target is an ordinary operator event that
/// the removal gate handles, while a lost probe stops the rig. `ArmError::Probe` is the layer
/// boundary — anything below it is USB or the probe firmware, anything above it is the wire.
fn is_probe_failure(err: &ArmError) -> bool {
    matches!(err, ArmError::Probe(_))
}

fn probe_gone(what: &str, err: impl std::fmt::Display) -> RigError {
    RigError::new(RigErrorKind::ProbeGone, format!("could not {what}: {err}"))
}

fn target_error(what: &str, err: ArmError) -> RigError {
    let kind = if is_probe_failure(&err) {
        RigErrorKind::ProbeGone
    } else {
        RigErrorKind::ContactLost
    };
    RigError::new(kind, format!("could not {what}: {err}"))
}

/// How long a caller should wait before deciding a halt failed. Not used until the write path
/// exists; named here so the constant has one home.
pub const HALT_TIMEOUT: Duration = Duration::from_millis(500);
