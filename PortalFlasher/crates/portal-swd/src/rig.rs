//! The seam between policy and hardware.
//!
//! [`Machine`](crate::Machine) decides *what* to do; a [`Rig`] does it. Keeping them apart is
//! what lets the interesting failures — a contact lost 60% of the way through programming, a
//! probe that disappears mid-erase, a board that flashes cleanly and then sits in a reset loop —
//! be tests rather than bench sessions.
//!
//! [`SimRig`] is not a stand-in for the real thing so much as a way to reach states the real
//! thing reaches rarely and destructively. Its flash array is real enough to assert on: after an
//! interrupted write you can look at it and see that it is half-erased, and after the re-flash
//! that follows, see that it is not.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::addr;
use crate::bits;
use crate::image::{ImageBundle, RunCheckSpec};

/// Whether a target answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    Present,
    Absent,
}

/// Why a rig operation failed. The kind is what the log keys on; the detail is what the
/// operator reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RigError {
    pub kind: RigErrorKind,
    pub detail: String,
}

impl RigError {
    pub fn new(kind: RigErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }

    /// Whether this error means the *probe* is gone, as opposed to the target.
    ///
    /// The distinction matters: a lost target is an operator event that the removal gate
    /// handles, while a lost probe is an equipment fault that stops the rig.
    pub fn is_probe_loss(&self) -> bool {
        self.kind == RigErrorKind::ProbeGone
    }
}

impl core::fmt::Display for RigError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.detail)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RigErrorKind {
    /// The probe stopped answering: USB dropout, driver unbound, unplugged.
    ProbeGone,
    /// The target stopped answering mid-operation. A hand lifted.
    ContactLost,
    /// Attached, but the part is not what the bundle is for.
    WrongTarget,
    /// Connect-under-reset did not actually reset. Erasing from here would be erasing a running
    /// target with a live watchdog, so it is a hard stop rather than a warning.
    ResetIneffective,
    /// Readout protection is on. Recovering means a mass erase, which is an explicit,
    /// operator-confirmed action and never automatic.
    ReadoutProtected,
    /// Erase or program reported an error.
    Program,
    /// The readback did not match the bundle.
    Verify,
    /// The option-byte sequence failed, or did not take.
    OptionBytes,
    /// The target is not running the application.
    NotRunning,
    /// A bundle that should never have been loaded.
    BadBundle,
}

/// What a probe is, for the log and the status bar.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProbeInfo {
    pub name: String,
    pub serial: Option<String>,
    pub firmware: Option<String>,
    pub speed_khz: u32,
}

/// Where a pass has got to, for the progress readout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Attach,
    OptionBytes,
    Erase,
    Program,
    Readback,
    ResetRun,
}

impl core::fmt::Display for Step {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Step::Attach => "attach",
            Step::OptionBytes => "option-bytes",
            Step::Erase => "erase",
            Step::Program => "program",
            Step::Readback => "readback",
            Step::ResetRun => "reset-run",
        })
    }
}

/// Evidence from a completed flash pass. Every field is here because the log should be able to
/// answer "what exactly did you put on that board" without the board.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlashReport {
    pub idcode: u32,
    pub optr_before: u32,
    pub optr_after: u32,
    pub option_bytes_programmed: bool,
    pub rcc_csr: u32,
    /// Hash of what was read back off the device, not of what was sent to it.
    pub readback_sha256: String,
}

/// Evidence from a run-check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunCheckReport {
    pub vtor: u32,
    pub dhcsr_first: u32,
    pub dhcsr_second: u32,
    pub liveness_first: u32,
    pub liveness_second: u32,
    pub rcc_csr: u32,
}

impl RunCheckReport {
    /// The predicate, in one place, so the rig and the tests cannot disagree about it.
    ///
    /// `VTOR` is an identity check rather than a liveness one: a board spinning in
    /// `HardFault_Handler` still has the application's vector table installed. It earns its place
    /// by catching the *other* failure — a board that came out of reset into the system ROM
    /// bootloader because `nBOOT_SEL` let BOOT0 come from the pin the probe drives.
    pub fn verdict(&self, spec: &RunCheckSpec) -> Result<(), RunCheckFault> {
        if self.vtor != spec.vtor {
            return Err(RunCheckFault::WrongVectorTable { found: self.vtor });
        }
        for dhcsr in [self.dhcsr_first, self.dhcsr_second] {
            if dhcsr & bits::DHCSR_S_LOCKUP != 0 {
                return Err(RunCheckFault::LockedUp);
            }
            if dhcsr & bits::DHCSR_S_HALT != 0 {
                return Err(RunCheckFault::Halted);
            }
            if dhcsr & bits::DHCSR_S_SLEEP != 0 {
                return Err(RunCheckFault::Asleep);
            }
        }
        // Sticky, and cleared by the first read. Set on the second means the part reset between
        // the samples -- a board in a watchdog loop looks perfectly alive on any single sample.
        if self.dhcsr_second & bits::DHCSR_S_RESET_ST != 0 {
            return Err(RunCheckFault::ResetDuringWindow);
        }
        // Not `>`: the counter wraps, and a wrap is a running board, not a stalled one.
        if self.liveness_first == self.liveness_second {
            return Err(RunCheckFault::NotAdvancing {
                value: self.liveness_first,
            });
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunCheckFault {
    WrongVectorTable { found: u32 },
    Halted,
    Asleep,
    LockedUp,
    ResetDuringWindow,
    NotAdvancing { value: u32 },
}

impl core::fmt::Display for RunCheckFault {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            RunCheckFault::WrongVectorTable { found } => write!(
                f,
                "VTOR is {found:#010X}; the application was not entered (a system-ROM boot reads \
                 around {:#010X})",
                0x1FFF_0000u32
            ),
            RunCheckFault::Halted => f.write_str("the core is halted"),
            RunCheckFault::Asleep => {
                f.write_str("the core is asleep, which this firmware never is")
            }
            RunCheckFault::LockedUp => f.write_str("the core is locked up"),
            RunCheckFault::ResetDuringWindow => f.write_str(
                "the target reset during the check -- a watchdog loop looks alive on \
                             any single sample",
            ),
            RunCheckFault::NotAdvancing { value } => {
                write!(f, "the liveness counter is stuck at {value}")
            }
        }
    }
}

/// Somewhere for a pass to report progress. Deliberately a callback rather than a channel, so
/// the rig has no opinion about how the caller is threaded.
pub type Progress<'a> = dyn FnMut(Step, u64, u64) + 'a;

/// A probe and the target in front of it.
///
/// Implemented by the real probe-rs backed rig and by [`SimRig`]. The state machine never sees
/// either; the worker owns one and calls it.
pub trait Rig: Send {
    fn open(&mut self) -> Result<ProbeInfo, RigError>;

    /// The cheap poll. Must not halt, reset, or write anything to the target.
    fn poll(&mut self) -> Result<Presence, RigError>;

    /// Read the whole device without halting or resetting it.
    ///
    /// This is what the operator's "Read device" does, and what the firmware map draws. It is on
    /// the trait rather than only on the real rig so the page behaves identically against a
    /// simulated target — a map that only worked with hardware attached would be untestable in
    /// exactly the situation it is most useful.
    fn read_device(&mut self) -> Result<crate::device::DeviceImage, RigError>;

    fn flash(
        &mut self,
        bundle: &ImageBundle,
        progress: &mut Progress<'_>,
    ) -> Result<FlashReport, RigError>;

    /// Attach without halting or resetting and prove the application is executing.
    fn run_check(&mut self, spec: &RunCheckSpec) -> Result<RunCheckReport, RigError>;

    fn close(&mut self);
}

// ---------------------------------------------------------------- the simulated target

/// Where a fault should be injected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    OnAttach,
    OnOptionBytes,
    /// Part-way through the erase, as a fraction.
    DuringErase(u8),
    /// Part-way through programming, as a percentage. The interesting one: it leaves the flash
    /// array visibly half-written.
    DuringProgram(u8),
    DuringReadback,
    OnRunCheck,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fault {
    pub at: Trigger,
    pub kind: RigErrorKind,
}

/// A modelled STM32G070, complete enough to be wrong in the ways a real one is.
#[derive(Clone, Debug)]
pub struct SimRig {
    /// 128 kB, erased.
    flash: Vec<u8>,
    optr: u32,
    /// Increments while the simulated application runs.
    liveness: u32,
    vtor: u32,
    dhcsr: u32,
    /// Whether a board is in the fixture.
    ///
    /// Shared rather than owned, so whoever is driving the simulation — a test, or a switch on
    /// the operator page — can seat and lift a board while the worker thread holds the rig.
    /// The alternative, downcasting a `dyn Rig` back to this type, would put a simulation-shaped
    /// hole in the trait that the real probe would have to answer.
    present: Arc<AtomicBool>,
    opened: bool,
    faults: Vec<Fault>,
    /// Set once a flash pass has completed, so the run-check has something true to say.
    programmed: bool,
}

impl Default for SimRig {
    fn default() -> Self {
        Self::new()
    }
}

impl SimRig {
    pub fn new() -> Self {
        Self {
            flash: vec![0xFF; (addr::FLASH_END - addr::FLASH_BASE) as usize],
            // A virgin part: ST's factory default.
            optr: 0xFFFF_FEAA,
            liveness: 0,
            vtor: 0,
            dhcsr: 0,
            present: Arc::new(AtomicBool::new(false)),
            opened: false,
            faults: Vec::new(),
            programmed: false,
        }
    }

    /// A handle on the fixture: seat and lift a board from anywhere, including while the worker
    /// thread owns the rig.
    pub fn fixture(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.present)
    }

    pub fn set_present(&self, present: bool) {
        self.present.store(present, Ordering::Relaxed);
    }

    pub fn is_present(&self) -> bool {
        self.present.load(Ordering::Relaxed)
    }

    pub fn with_fault(mut self, at: Trigger, kind: RigErrorKind) -> Self {
        self.faults.push(Fault { at, kind });
        self
    }

    /// Start from a part that already has an image on it, rather than a virgin one.
    pub fn preloaded(mut self, bundle: &ImageBundle) -> Self {
        self.flash = bundle.expected_flash_image();
        self.programmed = true;
        self.vtor = addr::APP_BASE;
        self
    }

    pub fn with_optr(mut self, optr: u32) -> Self {
        self.optr = optr;
        self
    }

    pub fn flash_bytes(&self) -> &[u8] {
        &self.flash
    }

    pub fn optr(&self) -> u32 {
        self.optr
    }

    /// How much of flash is not erased. The measure that makes an interrupted write visible.
    pub fn programmed_bytes(&self) -> usize {
        self.flash.iter().filter(|&&b| b != 0xFF).count()
    }

    fn trip(&self, at: Trigger) -> Option<RigError> {
        self.faults
            .iter()
            .find(|f| f.at == at)
            .map(|f| RigError::new(f.kind, format!("simulated fault at {:?}", f.at)))
    }
}

impl Rig for SimRig {
    fn open(&mut self) -> Result<ProbeInfo, RigError> {
        self.opened = true;
        Ok(ProbeInfo {
            name: "SimRig".into(),
            serial: Some("SIM".into()),
            firmware: Some("V2J37S7".into()),
            speed_khz: 1_800,
        })
    }

    fn poll(&mut self) -> Result<Presence, RigError> {
        if !self.opened {
            return Err(RigError::new(RigErrorKind::ProbeGone, "probe not open"));
        }
        let present = self.is_present();
        // A running application ticks whether or not anyone is polling it.
        if self.programmed && present {
            self.liveness = self.liveness.wrapping_add(7);
        }
        Ok(if present {
            Presence::Present
        } else {
            Presence::Absent
        })
    }

    fn read_device(&mut self) -> Result<crate::device::DeviceImage, RigError> {
        if !self.opened {
            return Err(RigError::new(RigErrorKind::ProbeGone, "probe not open"));
        }
        if !self.is_present() {
            return Err(RigError::new(
                RigErrorKind::ContactLost,
                "no target in the fixture",
            ));
        }
        Ok(crate::device::DeviceImage {
            flash: self.flash.clone(),
            optr: self.optr,
            idcode: Some(0x2001_6460),
            // A stable made-up id. Nothing keys on it, and a simulated run that produced a
            // plausible real UID would be worse than one that obviously did not.
            uid: [0x5111_0000, 0x5111_0001, 0x5111_0002],
            flash_kb: 128,
            rcc_csr: 0,
        })
    }

    fn flash(
        &mut self,
        bundle: &ImageBundle,
        progress: &mut Progress<'_>,
    ) -> Result<FlashReport, RigError> {
        if !self.opened {
            return Err(RigError::new(RigErrorKind::ProbeGone, "probe not open"));
        }
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

        progress(Step::Attach, 0, 1);
        if let Some(err) = self.trip(Trigger::OnAttach) {
            return Err(err);
        }
        if self.optr & bits::OPTR_RDP_MASK != bits::OPTR_RDP_LEVEL0 {
            return Err(RigError::new(
                RigErrorKind::ReadoutProtected,
                format!("OPTR reads {:#010X}; RDP is not level 0", self.optr),
            ));
        }

        // Option bytes first, so the reset OBL_LAUNCH causes cannot land mid-write.
        let optr_before = self.optr;
        let mut programmed_options = false;
        if bundle.option_bytes.needs_programming(self.optr) {
            progress(Step::OptionBytes, 0, 1);
            if let Some(err) = self.trip(Trigger::OnOptionBytes) {
                return Err(err);
            }
            self.optr = bundle.option_bytes.desired(self.optr);
            programmed_options = true;
        }

        // Erase.
        let total = self.flash.len() as u64;
        let erase_stop = self
            .faults
            .iter()
            .find_map(|f| match f.at {
                Trigger::DuringErase(pct) => Some(pct),
                _ => None,
            })
            .map(|pct| (total * u64::from(pct.min(100))) / 100);
        for (index, byte) in self.flash.iter_mut().enumerate() {
            if let Some(stop) = erase_stop
                && index as u64 >= stop
            {
                // Left half-erased on purpose. This is the state a lift mid-write produces, and
                // the thing the "never resume a partial write" rule exists for.
                self.programmed = false;
                self.vtor = 0;
                return Err(self.trip(Trigger::DuringErase(0)).unwrap_or_else(|| {
                    RigError::new(RigErrorKind::ContactLost, "erase interrupted")
                }));
            }
            *byte = 0xFF;
        }
        progress(Step::Erase, total, total);
        self.programmed = false;
        self.vtor = 0;

        // Program.
        let expected = bundle.expected_flash_image();
        let program_stop = self
            .faults
            .iter()
            .find_map(|f| match f.at {
                Trigger::DuringProgram(pct) => Some(pct),
                _ => None,
            })
            .map(|pct| (total * u64::from(pct.min(100))) / 100);
        for (index, byte) in expected.iter().enumerate() {
            if let Some(stop) = program_stop
                && index as u64 >= stop
            {
                return Err(self.trip(Trigger::DuringProgram(0)).unwrap_or_else(|| {
                    RigError::new(RigErrorKind::ContactLost, "programming interrupted")
                }));
            }
            self.flash[index] = *byte;
            if (index as u64).is_multiple_of(4096) {
                progress(Step::Program, index as u64, total);
            }
        }
        progress(Step::Program, total, total);

        // Readback, against the device rather than against what we meant to send.
        progress(Step::Readback, 0, total);
        if let Some(err) = self.trip(Trigger::DuringReadback) {
            return Err(err);
        }
        if self.flash != expected {
            let at = self
                .flash
                .iter()
                .zip(expected.iter())
                .position(|(a, b)| a != b)
                .unwrap_or(0);
            return Err(RigError::new(
                RigErrorKind::Verify,
                format!(
                    "readback differs at {:#010X}: expected {:#04X}, got {:#04X}",
                    addr::FLASH_BASE as usize + at,
                    expected[at],
                    self.flash[at]
                ),
            ));
        }
        progress(Step::Readback, total, total);

        progress(Step::ResetRun, 0, 1);
        self.programmed = true;
        self.vtor = addr::APP_BASE;
        self.dhcsr = 0;

        Ok(FlashReport {
            idcode: bits::DEV_ID_STM32G07X,
            optr_before,
            optr_after: self.optr,
            option_bytes_programmed: programmed_options,
            rcc_csr: 1 << 26, // PINRSTF, as a connect-under-reset should leave it
            // Of the array, not of the bundle. The field says "what was read back off the
            // device", and the real rig hashes exactly that -- a simulation that quietly hashed
            // the input instead would be the one place the two could never be compared.
            readback_sha256: crate::device::sha256_hex(&self.flash),
        })
    }

    fn run_check(&mut self, spec: &RunCheckSpec) -> Result<RunCheckReport, RigError> {
        if !self.opened {
            return Err(RigError::new(RigErrorKind::ProbeGone, "probe not open"));
        }
        if let Some(err) = self.trip(Trigger::OnRunCheck) {
            return Err(err);
        }
        let first = self.liveness;
        if self.programmed {
            // The window passes and the application keeps working.
            self.liveness = self.liveness.wrapping_add(spec.window_ms as u32);
        }
        Ok(RunCheckReport {
            vtor: self.vtor,
            dhcsr_first: self.dhcsr,
            dhcsr_second: self.dhcsr,
            liveness_first: first,
            liveness_second: self.liveness,
            rcc_csr: 0,
        })
    }

    fn close(&mut self) {
        self.opened = false;
    }
}
