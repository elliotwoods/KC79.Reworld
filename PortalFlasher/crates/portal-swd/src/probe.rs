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

use probe_rs::MemoryInterface;
use probe_rs::architecture::arm::{
    ArmDebugInterface, ArmError, FullyQualifiedApAddress, dp::DpAddress,
    sequences::DefaultArmSequence,
};
use probe_rs::probe::{DebugProbeSelector, Probe, WireProtocol, list::Lister};

use crate::addr;
use crate::device::DeviceImage;
use crate::image::{ImageBundle, RunCheckSpec};
use crate::rig::{
    FlashReport, Presence, ProbeInfo, Progress, Rig, RigError, RigErrorKind, RunCheckReport,
};

/// The chip name in probe-rs's built-in registry.
pub const TARGET: &str = "STM32G070RBTx";

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
                info.open().map_err(|err| probe_gone("open the probe", err))?
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
        _bundle: &ImageBundle,
        _progress: &mut Progress<'_>,
    ) -> Result<FlashReport, RigError> {
        // Deliberately not implemented yet rather than half-implemented. The write path is its own
        // step, it runs first on a scrap board, and a plausible-looking stub here is exactly the
        // thing that would get trusted by accident.
        Err(RigError::new(
            RigErrorKind::Program,
            "the write path is not implemented yet; this build can read a device but not \
             programme one",
        ))
    }

    fn run_check(&mut self, _spec: &RunCheckSpec) -> Result<RunCheckReport, RigError> {
        Err(RigError::new(
            RigErrorKind::NotRunning,
            "the run-check is not implemented yet",
        ))
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
