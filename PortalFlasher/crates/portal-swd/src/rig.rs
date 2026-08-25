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
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::addr;
use crate::bits;
use crate::image::{ImageBundle, RunCheckSpec};
use crate::persistent::DeviceSettings;

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
    /// Provisioning identity is corrupt, bound to another MCU, or conflicts with the requested
    /// PCB serial. This always needs an explicit operator resolution.
    IdentityConflict,
}

impl RigErrorKind {
    /// A stable name, for the log and for the page.
    pub fn as_str(self) -> &'static str {
        match self {
            RigErrorKind::ProbeGone => "probe-gone",
            RigErrorKind::ContactLost => "contact-lost",
            RigErrorKind::WrongTarget => "wrong-target",
            RigErrorKind::ResetIneffective => "reset-ineffective",
            RigErrorKind::ReadoutProtected => "readout-protected",
            RigErrorKind::Program => "program",
            RigErrorKind::Verify => "verify",
            RigErrorKind::OptionBytes => "option-bytes",
            RigErrorKind::NotRunning => "not-running",
            RigErrorKind::BadBundle => "bad-bundle",
            RigErrorKind::IdentityConflict => "identity-conflict",
        }
    }

    /// What to do about it, in one line an operator can act on without reading this crate.
    ///
    /// The `detail` on a [`RigError`] says what went wrong and is often a probe-rs message written
    /// for whoever wrote probe-rs. This says what to *do*, which is a different sentence and the
    /// one that is actually needed while a board is sitting in the fixture.
    pub fn advice(self) -> &'static str {
        match self {
            RigErrorKind::ProbeGone => {
                "Check the ST-Link's USB cable, then Rescan. If it moved ports, reselect it."
            }
            RigErrorKind::ContactLost => {
                "The target stopped answering. Reseat the board and check the SWD pins -- \
                 SWDIO/SWCLK, ground, and that the board has power."
            }
            RigErrorKind::WrongTarget => {
                "The part is not an STM32G070. Check the board is the one you meant to flash."
            }
            RigErrorKind::ResetIneffective => {
                "NRST did not reset the part. Check the reset line is wired to the probe; without \
                 it this refuses to erase rather than erasing a running target."
            }
            RigErrorKind::ReadoutProtected => {
                "Readout protection is on. Clearing it is a mass erase that destroys the \
                 contents, so it is never automatic -- use STM32CubeProgrammer deliberately."
            }
            RigErrorKind::Program => {
                "Erase or programming failed. The board is very likely half-written: reflash it \
                 before using it, and do not trust what is on it now."
            }
            RigErrorKind::Verify => {
                "The readback did not match what was sent. Try a slower probe speed, and reflash \
                 -- this board is not good."
            }
            RigErrorKind::OptionBytes => {
                "The option-byte write did not take. The flash contents are unaffected; the boot \
                 configuration may not be what was asked for."
            }
            RigErrorKind::NotRunning => {
                "Flashed and verified, but the application is not running. Power-cycle the board \
                 and read it back; if it stays dead the image may be linked for the wrong bank."
            }
            RigErrorKind::BadBundle => {
                "The selected image was refused before anything was written. The board is \
                 untouched. Pick a different artefact."
            }
            RigErrorKind::IdentityConflict => {
                "Resolve the on-board and PCB serial explicitly. Automatic flashing will not \
                 choose which identity wins."
            }
        }
    }

    /// Whether flash contents may have been changed before this failed.
    ///
    /// The single most useful thing to know at the moment a pass fails: it is the difference
    /// between "try again" and "that board is now half-written and must not leave the bench".
    pub fn may_have_written(self) -> bool {
        match self {
            // Nothing has been written yet at these points, or the write is to option flash only.
            RigErrorKind::ProbeGone
            | RigErrorKind::WrongTarget
            | RigErrorKind::ResetIneffective
            | RigErrorKind::ReadoutProtected
            | RigErrorKind::BadBundle
            | RigErrorKind::IdentityConflict
            | RigErrorKind::OptionBytes => false,
            // `ContactLost` is the ambiguous one and is deliberately counted as written: it can
            // arrive during the attach, when nothing has happened, or 60% of the way through
            // programming. Guessing wrong in the safe direction costs a needless reflash; guessing
            // wrong in the other direction ships a half-written board.
            RigErrorKind::ContactLost
            | RigErrorKind::Program
            | RigErrorKind::Verify
            | RigErrorKind::NotRunning => true,
        }
    }
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
    Identity,
    Settings,
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
            Step::Identity => "identity",
            Step::Settings => "settings",
        })
    }
}

/// What a programming step does with the core once it has finished writing.
///
/// A pass that programs firmware *and* durable records is two probe sessions, and each one used to
/// end by resetting the part and letting it go. The board therefore started its application once
/// per session — twice for a provisioning pass, and a third time if the caller then restarted it
/// deliberately — and on this product each start runs a homing routine, so the operator watched
/// the prisms home, stop, and home again. The intermediate starts were a property of how the rig
/// let go, not steps anybody asked for.
///
/// Naming it makes the restart the caller's decision instead of a side effect of session
/// lifetimes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Release {
    /// Reset the part and let the application run. The final step of a sequence does this, and it
    /// is the only thing that restarts the board.
    Run,
    /// Leave the core halted so a following step can write more without the application having
    /// started and stopped in between.
    ///
    /// The watchdog stays frozen and the part stays stopped until something attaches again, so a
    /// sequence that releases this way **must** still reach a [`Release::Run`] or an explicit
    /// [`Rig::reset_and_run`] — including down its failure path, or a board is left dead in the
    /// fixture with nothing on screen saying why.
    Halt,
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
    /// Hashes over the independently read-back durable partitions. They are evidence that the
    /// flash pass preserved both, not hashes inferred from bytes read before programming.
    pub identity_sha256: String,
    pub settings_sha256: String,
    /// Which banks this pass wrote: `"full"`, `"bootloader only"` or `"application only"`.
    pub scope: &'static str,
    /// The firmware spans this pass left alone, `(start, end)`, proved unchanged by reading them
    /// before programming and again after. Empty on a full pass, and on any pass that erased the
    /// bank it left out.
    pub preserved: Vec<(u32, u32)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersistentWriteReport {
    pub serial: u32,
    pub settings: DeviceSettings,
    pub identity_written: bool,
    pub settings_written: bool,
}

/// Non-invasive evidence that reset reached the application and left the core executing.
///
/// This deliberately does not require the main-loop liveness counter to advance. PortalFW runs
/// its long startup routine from `setup()`, before `loop()` increments that counter, so the full
/// run-check would call a correctly booting module dead for the whole startup sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootReport {
    pub vtor: u32,
    pub dhcsr_first: u32,
    pub dhcsr_second: u32,
    pub rcc_csr: u32,
}

impl BootReport {
    pub fn verdict(&self, expected_vtor: u32) -> Result<(), BootFault> {
        if self.vtor != expected_vtor {
            return Err(BootFault::WrongVectorTable { found: self.vtor });
        }
        for dhcsr in [self.dhcsr_first, self.dhcsr_second] {
            if dhcsr & bits::DHCSR_S_LOCKUP != 0 {
                return Err(BootFault::LockedUp);
            }
            if dhcsr & bits::DHCSR_S_HALT != 0 {
                return Err(BootFault::Halted);
            }
        }
        if self.dhcsr_second & bits::DHCSR_S_RESET_ST != 0 {
            return Err(BootFault::ResetDuringWindow);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootFault {
    WrongVectorTable { found: u32 },
    Halted,
    LockedUp,
    ResetDuringWindow,
}

impl core::fmt::Display for BootFault {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            BootFault::WrongVectorTable { found } => write!(
                f,
                "VTOR is {found:#010X}; reset did not enter the application"
            ),
            BootFault::Halted => f.write_str("the core is halted after reset"),
            BootFault::LockedUp => f.write_str("the core locked up after reset"),
            BootFault::ResetDuringWindow => {
                f.write_str("the target reset again during the boot check")
            }
        }
    }
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

    /// Program the firmware partition and verify it by readback.
    ///
    /// `release` decides whether the board starts afterwards. A pass that has nothing further to
    /// write passes [`Release::Run`]; one that goes on to write identity or settings passes
    /// [`Release::Halt`], so the application starts once, at the end, running what was written.
    fn flash(
        &mut self,
        bundle: &ImageBundle,
        release: Release,
        progress: &mut Progress<'_>,
    ) -> Result<FlashReport, RigError>;

    /// Append identity and journal settings. A conflicting/corrupt identity is changed only when
    /// `allow_identity_override` records an operator's explicit resolution.
    ///
    /// `release` means what it does on [`Rig::flash`].
    fn write_persistent(
        &mut self,
        serial: u32,
        settings: DeviceSettings,
        allow_identity_override: bool,
        release: Release,
        progress: &mut Progress<'_>,
    ) -> Result<PersistentWriteReport, RigError>;

    /// Assert reset, release the debugger's halt, and leave the MCU executing.
    fn reset_and_run(&mut self) -> Result<(), RigError>;

    /// Observe the application vector table and core state without halting or resetting it.
    fn boot_check(&mut self, expected_vtor: u32) -> Result<BootReport, RigError>;

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
    /// The erase runs past the window it was given, into a bank this pass promised to leave alone.
    ///
    /// Not a fault the *host* injects but one the target can produce: probe-rs erases whole
    /// sectors, and a staged range that were ever mis-computed -- or a flash algorithm that
    /// rounded outwards -- would take the neighbouring bank with it. Modelled because the
    /// preserved-range comparison is the only thing standing between that and a board whose
    /// firmware is silently gone, and a guard nothing can trip is a guard nobody is testing.
    EraseBeyondWindow,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fault {
    pub at: Trigger,
    pub kind: RigErrorKind,
}

/// Whether two triggers name the same injection point, ignoring how deep into it.
///
/// `DuringErase` and `DuringProgram` carry a percentage, and the erase and program loops construct
/// a zero-valued one to look their fault up by. Comparing the payload as well meant
/// `with_fault(DuringProgram(50), ProbeGone)` found nothing and fell back to a hard-coded
/// `ContactLost` — the *kind* the caller asked for was silently discarded, so every test built on
/// a mid-write injection was asserting against a different failure from the one it named.
fn same_site(a: Trigger, b: Trigger) -> bool {
    match (a, b) {
        (Trigger::DuringErase(_), Trigger::DuringErase(_)) => true,
        (Trigger::DuringProgram(_), Trigger::DuringProgram(_)) => true,
        _ => a == b,
    }
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
    /// Where the application this part would start actually sits, so the modelled `VTOR` is the
    /// one a real board would report rather than a constant. A board flashed with a legacy-base
    /// image comes up with `VTOR == 0x08006000`, and a simulation that always said `0x08004000`
    /// would make the run-check's whole purpose untestable.
    app_base: u32,
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
    /// How many times this rig has released the core to run the application.
    ///
    /// Shared for the same reason `present` is, and counted because "how many times did the board
    /// restart" is the property a sequence of programming steps is easiest to get wrong and
    /// hardest to see: every intermediate restart still ends in a board that is running, so
    /// nothing downstream looks wrong. It is a number, so a test can hold it to exactly one.
    starts: Arc<AtomicU32>,
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
            app_base: addr::APP_BASE,
            dhcsr: 0,
            present: Arc::new(AtomicBool::new(false)),
            opened: false,
            faults: Vec::new(),
            programmed: false,
            starts: Arc::new(AtomicU32::new(0)),
        }
    }

    /// A handle on the fixture: seat and lift a board from anywhere, including while the worker
    /// thread owns the rig.
    pub fn fixture(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.present)
    }

    /// A handle on the restart count, readable while the worker thread owns the rig.
    pub fn starts(&self) -> Arc<AtomicU32> {
        Arc::clone(&self.starts)
    }

    /// Reset the modelled part and let it run — the one thing that restarts a board, counted.
    fn release_to_run(&mut self) {
        self.dhcsr = 0;
        self.vtor = if self.programmed { self.app_base } else { 0 };
        self.starts.fetch_add(1, Ordering::Relaxed);
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
        let firmware = bundle.expected_firmware_image();
        self.flash[..firmware.len()].copy_from_slice(&firmware);
        self.programmed = true;
        self.app_base = bundle.application.load_address;
        self.vtor = self.app_base;
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
            .find(|f| same_site(f.at, at))
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
        release: Release,
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

        // Only the banks this pass writes, and never the final three pages: those are durable
        // identity/settings. A bank left out under `Unselected::Preserve` produces no window and
        // is therefore neither erased nor programmed -- which is the whole of "preserve", modelled
        // here the same way the real rig does it rather than asserted.
        let windows = bundle.write_windows();
        let preserved = bundle.preserved_windows();
        let offset_of = |address: u32| (address - addr::FLASH_BASE) as usize;
        let before: Vec<(u32, u32, Vec<u8>)> = preserved
            .iter()
            .map(|&(start, end)| {
                (
                    start,
                    end,
                    self.flash[offset_of(start)..offset_of(end)].to_vec(),
                )
            })
            .collect();

        let total: u64 = windows.iter().map(|w| w.bytes.len() as u64).sum();
        let erase_stop = self
            .faults
            .iter()
            .find_map(|f| match f.at {
                Trigger::DuringErase(pct) => Some(pct),
                _ => None,
            })
            .map(|pct| (total * u64::from(pct.min(100))) / 100);
        let mut erased = 0_u64;
        for window in &windows {
            let at = offset_of(window.start);
            for byte in &mut self.flash[at..at + window.bytes.len()] {
                if let Some(stop) = erase_stop
                    && erased >= stop
                {
                    // Left half-erased on purpose. This is the state a lift mid-write produces,
                    // and the thing the "never resume a partial write" rule exists for.
                    self.programmed = false;
                    self.vtor = 0;
                    return Err(self.trip(Trigger::DuringErase(0)).unwrap_or_else(|| {
                        RigError::new(RigErrorKind::ContactLost, "erase interrupted")
                    }));
                }
                *byte = 0xFF;
                erased += 1;
            }
        }
        // A modelled flash algorithm that does not respect the window it was handed. This does
        // not return an error: the point is that the pass proceeds, succeeds at everything it was
        // watching, and has to be caught by the preserved-range comparison alone.
        if self
            .faults
            .iter()
            .any(|f| f.at == Trigger::EraseBeyondWindow)
        {
            for &(start, end) in &preserved {
                for byte in &mut self.flash[offset_of(start)..offset_of(end)] {
                    *byte = 0xFF;
                }
            }
        }
        progress(Step::Erase, total, total);
        self.programmed = false;
        self.vtor = 0;

        // Program.
        let program_stop = self
            .faults
            .iter()
            .find_map(|f| match f.at {
                Trigger::DuringProgram(pct) => Some(pct),
                _ => None,
            })
            .map(|pct| (total * u64::from(pct.min(100))) / 100);
        let mut written = 0_u64;
        for window in &windows {
            let at = offset_of(window.start);
            for (index, byte) in window.bytes.iter().enumerate() {
                if let Some(stop) = program_stop
                    && written >= stop
                {
                    return Err(self.trip(Trigger::DuringProgram(0)).unwrap_or_else(|| {
                        RigError::new(RigErrorKind::ContactLost, "programming interrupted")
                    }));
                }
                self.flash[at + index] = *byte;
                written += 1;
                if written.is_multiple_of(4096) {
                    progress(Step::Program, written, total);
                }
            }
        }
        progress(Step::Program, total, total);

        // Readback, against the device rather than against what we meant to send.
        progress(Step::Readback, 0, total);
        if let Some(err) = self.trip(Trigger::DuringReadback) {
            return Err(err);
        }
        for window in &windows {
            let at = offset_of(window.start);
            let actual = &self.flash[at..at + window.bytes.len()];
            if let Some(index) = actual
                .iter()
                .zip(window.bytes.iter())
                .position(|(a, b)| a != b)
            {
                return Err(RigError::new(
                    RigErrorKind::Verify,
                    format!(
                        "readback differs at {:#010X}: expected {:#04X}, got {:#04X}",
                        window.start as usize + index,
                        window.bytes[index],
                        actual[index]
                    ),
                ));
            }
        }
        // A bank left out was neither erased nor programmed, so it must read exactly as it did
        // before the pass. Checked rather than assumed, for the same reason the real rig checks
        // it: a preserve nobody verified is indistinguishable from one that did not happen.
        for (start, end, was) in &before {
            let now = &self.flash[offset_of(*start)..offset_of(*end)];
            if let Some(index) = now.iter().zip(was.iter()).position(|(a, b)| a != b) {
                return Err(RigError::new(
                    RigErrorKind::Verify,
                    format!(
                        "the bank left out changed at {:#010X}: was {:#04X}, now {:#04X}",
                        *start as usize + index,
                        was[index],
                        now[index]
                    ),
                ));
            }
        }
        progress(Step::Readback, total, total);

        // Whether an application will run is a fact about the flash, not about whether this pass
        // happened to write one: a bootloader-only pass over a preserved application leaves a
        // board that still boots, and a pass that erased the bank leaves one that does not.
        //
        // Both bases are examined, in the order a v6 bootloader tries them, so a preserved
        // legacy-base application is still found after a pass that only wrote the bootloader.
        (self.programmed, self.app_base) = [addr::APP_BASE, addr::APP_BASE_LEGACY]
            .into_iter()
            .find(|base| {
                crate::device::VectorTable::read(&self.flash, (base - addr::FLASH_BASE) as usize)
                    .is_some()
            })
            .map_or((false, addr::APP_BASE), |base| (true, base));
        match release {
            Release::Run => {
                progress(Step::ResetRun, 0, 1);
                self.release_to_run();
            }
            // Halted, and staying that way: the caller has more to write, and the modelled part
            // must not run the application in between any more than the real one does.
            Release::Halt => self.dhcsr = bits::DHCSR_S_HALT,
        }

        Ok(FlashReport {
            idcode: bits::DEV_ID_STM32G07X,
            optr_before,
            optr_after: self.optr,
            option_bytes_programmed: programmed_options,
            rcc_csr: 1 << 26, // PINRSTF, as a connect-under-reset should leave it
            // Of the array, not of the bundle. The field says "what was read back off the
            // device", and the real rig hashes exactly that -- a simulation that quietly hashed
            // the input instead would be the one place the two could never be compared.
            readback_sha256: {
                let mut read = Vec::with_capacity(total as usize);
                for window in &windows {
                    let at = offset_of(window.start);
                    read.extend_from_slice(&self.flash[at..at + window.bytes.len()]);
                }
                crate::device::sha256_hex(&read)
            },
            identity_sha256: crate::device::sha256_hex(
                &self.flash[(addr::IDENTITY_BASE - addr::FLASH_BASE) as usize
                    ..(addr::SETTINGS_A_BASE - addr::FLASH_BASE) as usize],
            ),
            settings_sha256: crate::device::sha256_hex(
                &self.flash[(addr::SETTINGS_A_BASE - addr::FLASH_BASE) as usize..],
            ),
            scope: bundle.scope(),
            preserved,
        })
    }

    fn write_persistent(
        &mut self,
        serial: u32,
        mut settings: DeviceSettings,
        allow_identity_override: bool,
        release: Release,
        progress: &mut Progress<'_>,
    ) -> Result<PersistentWriteReport, RigError> {
        use crate::persistent::{
            IdentityRecord, IdentityState, JournalWrite, McuUid, SettingsRecord, SettingsSource,
            SettingsState, encode_identity, encode_settings, identity_write, scan_identity_page,
            settings_write,
        };
        if !self.opened || !self.is_present() {
            return Err(RigError::new(
                RigErrorKind::ContactLost,
                "no target in the fixture",
            ));
        }
        if serial == 0 || serial == u32::MAX || !settings.validate() {
            return Err(RigError::new(
                RigErrorKind::BadBundle,
                "invalid serial or settings",
            ));
        }
        let page = addr::FLASH_PAGE_BYTES as usize;
        let persist = (addr::PERSIST_BASE - addr::FLASH_BASE) as usize;
        let (identity_page, rest) = self.flash[persist..].split_at_mut(page);
        let (settings_a, settings_b) = rest.split_at_mut(page);
        let uid = McuUid([0x5111_0000, 0x5111_0001, 0x5111_0002]);
        let identity = scan_identity_page(identity_page, uid);
        let existing = identity.serial();
        if (matches!(
            identity,
            IdentityState::Corrupt | IdentityState::ForeignUid { .. }
        ) || existing.is_some_and(|value| value != serial))
            && !allow_identity_override
        {
            return Err(RigError::new(
                RigErrorKind::IdentityConflict,
                format!("on-board identity is {}", identity.name()),
            ));
        }

        let mut identity_written = false;
        if existing != Some(serial) {
            progress(Step::Identity, 0, 1);
            let generation = match identity {
                IdentityState::Valid { record } | IdentityState::ForeignUid { record } => {
                    record.generation.saturating_add(1)
                }
                _ => 1,
            };
            let Some(JournalWrite::Append { address }) = identity_write(identity_page) else {
                return Err(RigError::new(
                    RigErrorKind::IdentityConflict,
                    "identity journal is full",
                ));
            };
            let at = (address - addr::IDENTITY_BASE) as usize;
            identity_page[at..at + crate::persistent::RECORD_BYTES].copy_from_slice(
                &encode_identity(IdentityRecord {
                    generation,
                    uid,
                    serial,
                }),
            );
            identity_written = true;
            progress(Step::Identity, 1, 1);
        }

        let state = SettingsState::load(settings_a, settings_b, uid);
        if settings.axis_a_calibration.is_none() {
            settings.axis_a_calibration = state.record.settings.axis_a_calibration;
        }
        if settings.axis_b_calibration.is_none() {
            settings.axis_b_calibration = state.record.settings.axis_b_calibration;
        }
        let mut settings_written = false;
        if state.source == SettingsSource::Defaults || state.record.settings != settings {
            progress(Step::Settings, 0, 1);
            let record = encode_settings(SettingsRecord {
                generation: state.record.generation.saturating_add(1),
                uid,
                settings,
            });
            match settings_write(settings_a, settings_b, state.source) {
                JournalWrite::Append { address } => {
                    let (base, bytes) = if address >= addr::SETTINGS_B_BASE {
                        (addr::SETTINGS_B_BASE, settings_b)
                    } else {
                        (addr::SETTINGS_A_BASE, settings_a)
                    };
                    let at = (address - base) as usize;
                    bytes[at..at + record.len()].copy_from_slice(&record);
                }
                JournalWrite::Compact { page_address } => {
                    let bytes = if page_address == addr::SETTINGS_A_BASE {
                        settings_a
                    } else {
                        settings_b
                    };
                    bytes.fill(0xFF);
                    bytes[..record.len()].copy_from_slice(&record);
                }
            }
            settings_written = true;
            progress(Step::Settings, 1, 1);
        }
        // The real rig attaches under reset for this and releases on the way out, whether or not
        // it ended up writing anything. Model both, or the restart count the sim reports would be
        // lower than the one the operator watches.
        match release {
            Release::Run => self.release_to_run(),
            Release::Halt => self.dhcsr = bits::DHCSR_S_HALT,
        }
        Ok(PersistentWriteReport {
            serial,
            settings,
            identity_written,
            settings_written,
        })
    }

    fn reset_and_run(&mut self) -> Result<(), RigError> {
        if !self.opened {
            return Err(RigError::new(RigErrorKind::ProbeGone, "probe not open"));
        }
        if !self.is_present() {
            return Err(RigError::new(
                RigErrorKind::ContactLost,
                "no target in the fixture",
            ));
        }
        self.release_to_run();
        Ok(())
    }

    fn boot_check(&mut self, _expected_vtor: u32) -> Result<BootReport, RigError> {
        if !self.opened {
            return Err(RigError::new(RigErrorKind::ProbeGone, "probe not open"));
        }
        if !self.is_present() {
            return Err(RigError::new(
                RigErrorKind::ContactLost,
                "no target in the fixture",
            ));
        }
        Ok(BootReport {
            vtor: self.vtor,
            dhcsr_first: self.dhcsr,
            dhcsr_second: self.dhcsr,
            rcc_csr: 0,
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

#[cfg(test)]
mod tests {
    use super::{BootFault, BootReport, DeviceSettings, Release, Rig, RigErrorKind, SimRig};
    use std::sync::atomic::Ordering;

    const APP_VTOR: u32 = 0x0800_8000;
    const DHCSR_HALT: u32 = 1 << 17;
    const DHCSR_RESET_ST: u32 = 1 << 25;

    /// An application image that says which bank it was linked for, the way a real one does.
    fn application_image(base: u32, len: usize) -> Vec<u8> {
        use crate::addr;
        let mut bytes = vec![0u8; len];
        bytes[0..4].copy_from_slice(&0x2000_9000u32.to_le_bytes());
        bytes[4..8].copy_from_slice(&((base + 0x240) | 1).to_le_bytes());
        let at = addr::APP_DESCRIPTOR_OFFSET;
        bytes[at..at + 8].copy_from_slice(addr::APP_DESCRIPTOR_MAGIC);
        bytes[at + 8..at + 12].copy_from_slice(&base.to_le_bytes());
        bytes
    }

    /// A bundle that passes `validate`, so a pass over it reaches the release rather than being
    /// refused before the probe is touched. Shaped like the flasher's `synthetic_bundle`: a v6
    /// bootloader and an application linked for the base that pairs with it.
    fn flashable_bundle() -> crate::image::ImageBundle {
        bundle_for(crate::addr::APP_BASE, 14_000)
    }

    /// The same, for either generation: `boot_bytes` has to fit below `base` or the pair is the
    /// one `validate` refuses.
    fn bundle_for(base: u32, boot_bytes: usize) -> crate::image::ImageBundle {
        use crate::addr;
        use crate::image::{OptionBytePolicy, Provenance, Region, RegionName, RunCheckSpec};

        crate::image::ImageBundle {
            bootloader: Region::new(
                RegionName::Bootloader,
                addr::FLASH_BASE,
                vec![0xA5; boot_bytes],
            ),
            application: Region::new(
                RegionName::Application,
                base,
                application_image(base, 60_000),
            ),
            option_bytes: OptionBytePolicy::default(),
            run_check: RunCheckSpec::for_base(base),
            provenance: Provenance::Synthetic,
            unselected: crate::image::Unselected::Erase,
        }
    }

    /// Preserve has to actually preserve, and the only way to know is to look at the bytes.
    ///
    /// A board is programmed in full, then flashed again with the application left out. What the
    /// application bank held has to survive verbatim, and so do the three durable pages.
    #[test]
    fn a_bootloader_only_pass_leaves_the_application_bank_exactly_as_it_found_it() {
        use crate::addr;
        use crate::image::{Region, RegionName, Unselected};

        let full = flashable_bundle();
        let mut rig = SimRig::new().preloaded(&full);
        rig.set_present(true);
        rig.open().unwrap();
        let app_before = rig.flash_bytes()[(addr::APP_BASE - addr::FLASH_BASE) as usize
            ..(addr::PERSIST_BASE - addr::FLASH_BASE) as usize]
            .to_vec();
        let durable_before =
            rig.flash_bytes()[(addr::PERSIST_BASE - addr::FLASH_BASE) as usize..].to_vec();

        let mut boot_only = flashable_bundle();
        boot_only.unselected = Unselected::Preserve;
        boot_only.bootloader =
            Region::new(RegionName::Bootloader, addr::FLASH_BASE, vec![0x5A; 12_000]);
        boot_only.application = Region::new(RegionName::Application, addr::APP_BASE, Vec::new());

        let report = rig
            .flash(&boot_only, Release::Run, &mut |_, _, _| {})
            .expect("a bootloader-only pass must be flashable");
        assert_eq!(report.scope, "bootloader only");
        assert_eq!(report.preserved, vec![(addr::APP_BASE, addr::PERSIST_BASE)]);

        let after = rig.flash_bytes();
        assert_eq!(
            after[(addr::APP_BASE - addr::FLASH_BASE) as usize
                ..(addr::PERSIST_BASE - addr::FLASH_BASE) as usize],
            app_before[..],
            "the application bank was left out, so not one byte of it may have moved"
        );
        assert_eq!(
            after[(addr::PERSIST_BASE - addr::FLASH_BASE) as usize..],
            durable_before[..],
            "identity and settings are never in range of a firmware pass"
        );
        assert_eq!(
            &after[..12_000],
            &vec![0x5A; 12_000][..],
            "the new bootloader"
        );
        assert!(
            after[12_000..(addr::APP_BASE - addr::FLASH_BASE) as usize]
                .iter()
                .all(|&b| b == 0xFF),
            "a shorter bootloader must not leave the old one's tail behind"
        );
    }

    /// A board still on v4/v5: the application sits at `0x08006000`, and everything a pass does
    /// has to follow it there rather than assume the v6 bank.
    #[test]
    fn a_legacy_base_bundle_is_written_and_started_at_the_legacy_base() {
        use crate::addr;
        use crate::image::{Region, RegionName, Unselected};

        let legacy = bundle_for(addr::APP_BASE_LEGACY, 22_708);
        assert_eq!(legacy.validate(), vec![], "a matched v4/v5 pair is valid");

        let mut rig = SimRig::new();
        rig.set_present(true);
        rig.open().unwrap();
        rig.flash(&legacy, Release::Run, &mut |_, _, _| {})
            .expect("a legacy-base pair must be flashable");

        assert_eq!(
            rig.boot_check(addr::APP_BASE_LEGACY).unwrap().vtor,
            addr::APP_BASE_LEGACY,
            "the board comes up on the vector table it was actually given"
        );
        assert_eq!(
            &rig.flash_bytes()[(addr::APP_BASE_LEGACY - addr::FLASH_BASE) as usize
                ..(addr::APP_BASE_LEGACY - addr::FLASH_BASE) as usize + 8],
            &legacy.application.bytes[..8],
            "the application was programmed at its own base, not at the v6 one"
        );

        // And a bootloader-only pass over it preserves the bank where that application actually
        // is: the boundary comes from the bootloader's size, which is what makes 0x08004000..
        // 0x08006000 part of the bootloader's bank on this board.
        let mut boot_only = bundle_for(addr::APP_BASE_LEGACY, 22_708);
        boot_only.unselected = Unselected::Preserve;
        boot_only.application =
            Region::new(RegionName::Application, addr::APP_BASE_LEGACY, Vec::new());
        let report = rig
            .flash(&boot_only, Release::Run, &mut |_, _, _| {})
            .expect("flashable");
        assert_eq!(
            report.preserved,
            vec![(addr::APP_BASE_LEGACY, addr::PERSIST_BASE)]
        );
        assert_eq!(
            rig.boot_check(addr::APP_BASE_LEGACY).unwrap().vtor,
            addr::APP_BASE_LEGACY,
            "the preserved legacy application is still what the board runs"
        );
    }

    /// The pairing that bricks a board, refused before the probe is touched.
    #[test]
    fn a_legacy_bootloader_beside_a_new_base_application_is_refused_by_the_rig() {
        use crate::addr;
        use crate::image::{BundleFault, Region, RegionName};

        let mut brick = flashable_bundle();
        // 22,708 bytes occupies eleven pages, so it reaches 0x08006000 -- over the top of an
        // application based at 0x08004000, vector table first.
        brick.bootloader =
            Region::new(RegionName::Bootloader, addr::FLASH_BASE, vec![0xA5; 22_708]);
        assert!(
            brick
                .validate()
                .contains(&BundleFault::BootloaderOverlapsApplication {
                    bootloader_end: addr::APP_BASE_LEGACY,
                    app_base: addr::APP_BASE,
                })
        );

        let mut rig = SimRig::new();
        rig.set_present(true);
        rig.open().unwrap();
        let error = rig
            .flash(&brick, Release::Run, &mut |_, _, _| {})
            .expect_err("the rig re-validates, so this must never reach the flash algorithm");
        assert_eq!(error.kind, RigErrorKind::BadBundle);
    }

    /// The brick this makes impossible.
    ///
    /// An application-only selection used to erase the bootloader bank -- documented, deliberate,
    /// and productive of a board with an application at `0x08006000`, nothing at the reset vector
    /// the core actually starts from, and no way back in except SWD.
    #[test]
    fn an_application_only_pass_leaves_the_bootloader_that_makes_the_board_bootable() {
        use crate::addr;
        use crate::image::{Region, RegionName, Unselected};

        let full = flashable_bundle();
        let mut rig = SimRig::new().preloaded(&full);
        rig.set_present(true);
        rig.open().unwrap();
        let boot_before =
            rig.flash_bytes()[..(addr::APP_BASE - addr::FLASH_BASE) as usize].to_vec();

        let mut app_only = flashable_bundle();
        app_only.unselected = Unselected::Preserve;
        app_only.bootloader = Region::new(RegionName::Bootloader, addr::FLASH_BASE, Vec::new());

        let report = rig
            .flash(&app_only, Release::Run, &mut |_, _, _| {})
            .expect("an application-only pass must be flashable");
        assert_eq!(report.scope, "application only");
        assert_eq!(report.preserved, vec![(addr::FLASH_BASE, addr::APP_BASE)]);
        assert_eq!(
            &rig.flash_bytes()[..(addr::APP_BASE - addr::FLASH_BASE) as usize],
            &boot_before[..],
            "the bootloader is what the board boots from; it must survive"
        );
    }

    /// The old behaviour stays reachable, and stays exactly what it was.
    #[test]
    fn erasing_the_bank_left_out_still_blanks_it() {
        use crate::addr;
        use crate::image::{Region, RegionName, Unselected};

        let full = flashable_bundle();
        let mut rig = SimRig::new().preloaded(&full);
        rig.set_present(true);
        rig.open().unwrap();

        let mut boot_only = flashable_bundle();
        boot_only.unselected = Unselected::Erase;
        boot_only.application = Region::new(RegionName::Application, addr::APP_BASE, Vec::new());

        let report = rig
            .flash(&boot_only, Release::Run, &mut |_, _, _| {})
            .expect("erasing the bank left out is still a pass");
        assert!(report.preserved.is_empty());
        assert!(
            rig.flash_bytes()[(addr::APP_BASE - addr::FLASH_BASE) as usize
                ..(addr::PERSIST_BASE - addr::FLASH_BASE) as usize]
                .iter()
                .all(|&b| b == 0xFF),
            "the bank left out was erased, as asked"
        );
    }

    /// The guard that makes "preserve" a claim rather than a hope.
    ///
    /// Everything else about a bootloader-only pass can be right -- the correct bytes staged, the
    /// correct window erased, a clean readback of everything the pass wrote -- while the bank it
    /// promised to leave alone has quietly gone. Nothing in the written half of the pass can see
    /// that. Only reading the other bank before and after can, which is why that comparison
    /// exists; this is what fails if anyone removes it.
    #[test]
    fn a_pass_that_erases_past_its_window_is_caught_by_the_preserved_range_comparison() {
        use crate::addr;
        use crate::image::{Region, RegionName, Unselected};
        use crate::rig::Trigger;

        let mut rig = SimRig::new()
            .preloaded(&flashable_bundle())
            .with_fault(Trigger::EraseBeyondWindow, RigErrorKind::Verify);
        rig.set_present(true);
        rig.open().unwrap();

        let mut boot_only = flashable_bundle();
        boot_only.unselected = Unselected::Preserve;
        boot_only.application = Region::new(RegionName::Application, addr::APP_BASE, Vec::new());

        let error = rig
            .flash(&boot_only, Release::Run, &mut |_, _, _| {})
            .expect_err("an erase that reached the preserved bank must fail the pass");
        assert_eq!(error.kind, RigErrorKind::Verify);
        assert!(
            error.detail.contains("the bank left out changed"),
            "the operator has to be told which promise was broken, not just that one was: {}",
            error.detail
        );
        assert!(
            error.detail.contains("08004000"),
            "and where: {}",
            error.detail
        );
    }

    /// Whether a board will boot is a fact about its flash, not about what this pass wrote.
    ///
    /// A bootloader-only pass over a preserved application leaves a board that still starts its
    /// application; the same pass over a board with an empty bank does not. The bench reads this
    /// to decide what to prove after the write, so the simulation has to model it rather than
    /// report "I programmed something".
    #[test]
    fn a_preserved_application_still_boots_and_an_erased_one_does_not() {
        use crate::addr;
        use crate::image::{Region, RegionName, Unselected};

        let mut boot_only = flashable_bundle();
        boot_only.application = Region::new(RegionName::Application, addr::APP_BASE, Vec::new());

        let mut kept = SimRig::new().preloaded(&flashable_bundle());
        kept.set_present(true);
        kept.open().unwrap();
        boot_only.unselected = Unselected::Preserve;
        kept.flash(&boot_only, Release::Run, &mut |_, _, _| {})
            .expect("flashable");
        assert_eq!(
            kept.boot_check(addr::APP_BASE).unwrap().vtor,
            addr::APP_BASE,
            "the preserved application is still what the board runs"
        );

        let mut blanked = SimRig::new().preloaded(&flashable_bundle());
        blanked.set_present(true);
        blanked.open().unwrap();
        boot_only.unselected = Unselected::Erase;
        blanked
            .flash(&boot_only, Release::Run, &mut |_, _, _| {})
            .expect("flashable");
        assert_eq!(
            blanked.boot_check(addr::APP_BASE).unwrap().vtor,
            0,
            "with the bank erased there is no application to arrive at"
        );
    }

    #[test]
    fn boot_report_accepts_running_application() {
        let report = BootReport {
            vtor: APP_VTOR,
            dhcsr_first: 0,
            dhcsr_second: 0,
            rcc_csr: 0,
        };

        assert_eq!(report.verdict(APP_VTOR), Ok(()));
    }

    #[test]
    fn boot_report_rejects_halt_and_reset_loop() {
        let halted = BootReport {
            vtor: APP_VTOR,
            dhcsr_first: DHCSR_HALT,
            dhcsr_second: DHCSR_HALT,
            rcc_csr: 0,
        };
        assert!(matches!(halted.verdict(APP_VTOR), Err(BootFault::Halted)));

        let resetting = BootReport {
            vtor: APP_VTOR,
            dhcsr_first: 0,
            dhcsr_second: DHCSR_RESET_ST,
            rcc_csr: 0,
        };
        assert!(matches!(
            resetting.verdict(APP_VTOR),
            Err(BootFault::ResetDuringWindow)
        ));
    }

    #[test]
    fn simulated_identity_is_durable_and_overrides_are_explicit() {
        let mut rig = SimRig::new();
        rig.fixture()
            .store(true, std::sync::atomic::Ordering::Relaxed);
        rig.open().unwrap();
        let settings = DeviceSettings::default();
        let mut progress = |_, _, _| {};
        let first = rig
            .write_persistent(41, settings, false, Release::Run, &mut progress)
            .unwrap();
        assert!(first.identity_written);
        assert!(first.settings_written);
        let report = rig.read_device().unwrap().analyse();
        assert_eq!(report.identity.serial(), Some(41));
        assert_eq!(report.settings.record.settings, settings);

        let conflict = rig
            .write_persistent(42, settings, false, Release::Run, &mut progress)
            .unwrap_err();
        assert_eq!(conflict.kind, RigErrorKind::IdentityConflict);
        rig.write_persistent(42, settings, true, Release::Run, &mut progress)
            .unwrap();
        assert_eq!(
            rig.read_device().unwrap().analyse().identity.serial(),
            Some(42)
        );
    }

    /// A provisioning pass restarts the board once, not once per session.
    ///
    /// The bug this holds shut was visible from across the room: this product homes its prisms on
    /// startup, so every restart is ten seconds of motion. Programming firmware and then writing
    /// the identity journal are two probe sessions, each of which used to reset and let go, so the
    /// board homed, stopped mid-travel, and homed again — and with a deliberate restart on the end
    /// of the pass, a third time.
    ///
    /// Nothing downstream noticed, which is why it needs a test rather than an assertion: every
    /// intermediate restart still leaves a board that is running the right image, so the pass
    /// passed, the readback matched and the boot check was true.
    #[test]
    fn a_two_stage_pass_starts_the_application_exactly_once() {
        let bundle = flashable_bundle();
        let settings = DeviceSettings::default();
        let mut progress = |_, _, _| {};

        let mut rig = SimRig::new();
        let starts = rig.starts();
        rig.fixture().store(true, Ordering::Relaxed);
        rig.open().unwrap();

        rig.flash(&bundle, Release::Halt, &mut progress).unwrap();
        assert_eq!(
            starts.load(Ordering::Relaxed),
            0,
            "a halted release must not start the application"
        );
        assert!(
            rig.boot_check(0).unwrap().dhcsr_first & super::bits::DHCSR_S_HALT != 0,
            "the part is halted between the stages of a pass, not running"
        );

        rig.write_persistent(7, settings, false, Release::Halt, &mut progress)
            .unwrap();
        assert_eq!(
            starts.load(Ordering::Relaxed),
            0,
            "nor may the durable-record write, whether or not it wrote anything"
        );

        rig.reset_and_run().unwrap();
        assert_eq!(
            starts.load(Ordering::Relaxed),
            1,
            "the board starts once, at the end, running everything that was written"
        );

        // And the shape that produced the complaint, kept here so the number has a comparison:
        // released to run at every stage, the same pass starts the board three times.
        let mut old = SimRig::new();
        let old_starts = old.starts();
        old.fixture().store(true, Ordering::Relaxed);
        old.open().unwrap();
        old.flash(&bundle, Release::Run, &mut progress).unwrap();
        old.write_persistent(7, settings, false, Release::Run, &mut progress)
            .unwrap();
        old.reset_and_run().unwrap();
        assert_eq!(old_starts.load(Ordering::Relaxed), 3);
    }
}
