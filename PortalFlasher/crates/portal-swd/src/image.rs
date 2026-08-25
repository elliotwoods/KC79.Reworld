//! What gets flashed, and everything about it that is not bytes.
//!
//! A bundle is two binaries and a manifest:
//!
//! ```text
//! images/portal-2026-08-17/
//!   manifest.json
//!   bootloader.bin      -> 0x08000000, <= 16 kB (v6) or <= 24 kB (v4/v5)
//!   application.bin     -> 0x08004000 (v6) or 0x08006000 (v4/v5)
//! ```
//!
//! Two binaries rather than one merged blob, because the RS485 field-update path can only ever
//! ship `application.bin`, and it needs it as a bare image starting at offset zero — which is
//! exactly what `FWUpdate::uploadFirmware` and `fw_update::upload` consume. A merged image is
//! useless to that path.
//!
//! The manifest carries the things a bare `.bin` cannot. Chief among them the load address, which
//! is now genuinely two addresses — see below — rather than a magic number that could be assumed.
//! Here it is data, and [`ImageBundle::validate`] is the thing that compares it with everything
//! else in the bundle.
//!
//! # Both regions, always
//!
//! A virgin part has `0xFFFFFFFF` at `0x08000000`, so its initial stack pointer and reset vector
//! are garbage and the core locks up the instant it leaves reset. Nothing ever reaches the
//! application, because the thing that jumps to it is the bootloader. So a bundle that could only
//! describe the application would be unable to bring up a new board at all — which is precisely
//! the job.
//!
//! # Two bases, and the one pairing that bricks a board
//!
//! An application is linked either for `0x08004000` (a v6 bootloader, 16 kB) or for `0x08006000`
//! (v4/v5, 24 kB). Both are current. The dangerous combination is not "the wrong application" but
//! a *mismatched pair*: a 24 kB bootloader written beside an application based at `0x08004000`
//! overwrites that application's first four pages — vector table included — with its own tail. The
//! board then has a bootloader that starts and an application that cannot, and no field-update path
//! to fix it with, because the bootloader is what the field-update path talks to.
//! [`BundleFault::BootloaderOverlapsApplication`] is that refusal.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::addr;
use crate::bits;

// ---------------------------------------------------------------- what an image says about itself

/// The descriptor an application image carries at `base + 0xC0`, stating which bank it was linked
/// for.
///
/// # Why an image has to say this outright
///
/// An application built for `0x08004000` and one built for `0x08006000` are, byte for byte, both
/// plausible Cortex-M images: a stack pointer in SRAM, a reset vector with the Thumb bit set inside
/// the application bank, then code. The banks overlap, so even the reset vector cannot separate
/// them — a new-base image's entry point routinely lands above `0x08006000` too. Nothing
/// distinguishes the two until an absolute address is dereferenced, at which point the wrong one
/// hard-faults somewhere unrelated to the mistake. With both builds sitting in `.pio/build` under
/// names differing by a suffix, that is not a failure worth having available.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppDescriptor {
    /// The address this image was linked for.
    pub base: u32,
    pub flags: u32,
    /// `PORTAL_VERSION_STRING`, NUL-trimmed.
    pub version: String,
}

/// Where an image's base address came from, which decides how much it can be trusted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BaseSource {
    /// Stated by the image itself.
    Descriptor,
    /// Inferred from the reset vector, for an image built before descriptors existed.
    InferredLegacy,
}

/// Read the descriptor at [`addr::APP_DESCRIPTOR_OFFSET`], if the image has one.
///
/// `None` covers both "built before descriptors existed" and "the magic does not match", which are
/// deliberately the same answer: a damaged descriptor must never be read as a valid one.
pub fn read_descriptor(image: &[u8]) -> Option<AppDescriptor> {
    let at = addr::APP_DESCRIPTOR_OFFSET;
    let bytes = image.get(at..at + addr::APP_DESCRIPTOR_BYTES)?;
    if &bytes[..8] != addr::APP_DESCRIPTOR_MAGIC {
        return None;
    }
    let word = |offset: usize| {
        u32::from_le_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ])
    };
    let version = &bytes[16..16 + addr::APP_VERSION_BYTES];
    let end = version
        .iter()
        .position(|b| *b == 0)
        .unwrap_or(version.len());
    Some(AppDescriptor {
        base: word(8),
        flags: word(12),
        version: String::from_utf8_lossy(&version[..end]).into_owned(),
    })
}

/// The image's initial stack pointer and reset vector, as linked.
pub fn vector_table(image: &[u8]) -> Option<(u32, u32)> {
    let head = image.get(..8)?;
    Some((
        u32::from_le_bytes([head[0], head[1], head[2], head[3]]),
        u32::from_le_bytes([head[4], head[5], head[6], head[7]]),
    ))
}

/// Establish which bank an application image was built for, or `None` if it cannot be established.
///
/// The descriptor wins whenever there is one. Without one the only conclusion available is
/// [`addr::APP_BASE_LEGACY`], and only when the reset vector actually lands inside the legacy bank:
/// an image predating the descriptor cannot have been built for the new base, because the new base
/// did not exist yet. So inference can never conclude "new base" — it would be a guess, and the
/// guess that is wrong produces a board that programs, verifies and hard-faults.
///
/// An image linked at `0x08000000` (the `no_bootloader` builds) reaches neither branch and is
/// refused, the same refusal `tools/firmware.mjs` and `router-proto`'s `app_image` apply for the
/// same reason.
pub fn image_base(image: &[u8]) -> Option<(u32, BaseSource)> {
    let (_, reset) = vector_table(image)?;
    if let Some(descriptor) = read_descriptor(image) {
        return addr::is_app_base(descriptor.base)
            .then_some((descriptor.base, BaseSource::Descriptor));
    }
    let entry = reset & !1;
    (reset & 1 == 1 && (addr::APP_BASE_LEGACY..addr::PERSIST_BASE).contains(&entry))
        .then_some((addr::APP_BASE_LEGACY, BaseSource::InferredLegacy))
}

/// How a pass writes a board, as a fact the UI can read rather than a sentence it asserts.
///
/// These three values are consumed by `ProbeRsRig::flash` when it builds probe-rs's
/// `DownloadOptions`, and read by the worker when it publishes `/setup/*`. That is the whole point
/// of them being here: a settings page saying "erases the whole chip" is worthless if it is a
/// string in the operator app describing a literal in the rig crate, because nothing then forces
/// the two to agree. The readout and the behaviour now come from the same constant.
///
/// They are constants and not parameters. Each one is load-bearing for a promise made elsewhere,
/// and making them settable would let an operator produce a board whose firmware map is a lie.
pub mod strategy {
    /// Whole-chip erase is forbidden: the final three pages are durable device state.
    pub const CHIP_ERASE: bool = false;

    /// Do not read-modify-write the sectors that are not being programmed.
    pub const KEEP_UNWRITTEN: bool = false;

    /// Leave probe-rs's own verify off.
    ///
    /// Not because verification is skipped — the opposite. probe-rs's verify is a second pass
    /// through the flash algorithm; the pass instead reads firmware and durable pages back as
    /// plain memory and compares them independently, which is both stricter and *evidence*, because it produces the
    /// bytes that get hashed into `FlashReport::readback_sha256`.
    pub const PROBE_RS_VERIFY: bool = false;

    /// The one-line description each of these earns, for the settings readout.
    pub fn erase() -> &'static str {
        if CHIP_ERASE {
            "whole chip (invalid for provisioned boards)"
        } else {
            "firmware pages only; identity and settings preserved"
        }
    }

    pub fn verify() -> &'static str {
        if PROBE_RS_VERIFY {
            "the flash algorithm's own verify pass"
        } else {
            "firmware and persistent records read back independently"
        }
    }
}

/// What a pass does about a bank the operator left out.
///
/// This is the one thing about the firmware map that is genuinely a choice, which is why it lives
/// beside [`strategy`] rather than in it: those are constants because each is load-bearing for a
/// promise, and this one is a promise the operator gets to make.
///
/// Before it existed, a bank that was not selected was always erased, and that was documented as
/// deliberate -- the map must not lie about the board. It still does not: [`Preserve`] does not
/// *claim* the other bank is unchanged, it reads it before programming and proves it byte for byte
/// afterwards, exactly as the durable pages are already proved. What changes is that "flash the
/// bootloader and leave the application alone" is now expressible, and that "flash the application
/// only" no longer erases the bootloader and hands back a board that cannot boot at all.
///
/// [`Preserve`]: Unselected::Preserve
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unselected {
    /// Erase and program only the selected bank's pages, and prove the rest is byte-identical to
    /// what the board already held.
    #[default]
    Preserve,
    /// Erase it. What every pass did before this existed, and still what a pass does when the
    /// operator wants a board carrying nothing but the image they chose.
    Erase,
}

impl Unselected {
    /// For the settings readout, in the same voice as [`strategy::erase`].
    pub fn as_str(self) -> &'static str {
        match self {
            Unselected::Preserve => "preserved, and proved unchanged by readback",
            Unselected::Erase => "erased",
        }
    }
}

/// A contiguous, page-aligned span this pass will erase, program and read back.
///
/// Page-aligned at *both* ends, and that is not incidental. probe-rs pads a partially filled page
/// with the erased byte value, so a window that stopped mid-page would quietly erase the rest of
/// that page -- which for the application bank is the difference between preserving a board's
/// firmware and destroying its first 2 kB. [`ImageBundle::write_windows`] only ever produces bank
/// boundaries, and `write_windows_are_page_aligned_and_stop_below_the_durable_pages` is what keeps
/// it that way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    pub start: u32,
    pub bytes: Vec<u8>,
}

impl Window {
    /// One past the last byte this window covers.
    pub fn end(&self) -> u32 {
        self.start + self.bytes.len() as u32
    }
}

/// A contiguous run of bytes and where it belongs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Region {
    pub name: RegionName,
    pub load_address: u32,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegionName {
    Bootloader,
    Application,
}

impl RegionName {
    pub fn as_str(self) -> &'static str {
        match self {
            RegionName::Bootloader => "bootloader",
            RegionName::Application => "application",
        }
    }
}

impl Region {
    pub fn new(name: RegionName, load_address: u32, bytes: Vec<u8>) -> Self {
        Self {
            name,
            load_address,
            bytes,
        }
    }

    pub fn sha256(&self) -> String {
        hex(&Sha256::digest(&self.bytes))
    }

    /// One past the last byte this region occupies.
    pub fn end_address(&self) -> u64 {
        u64::from(self.load_address) + self.bytes.len() as u64
    }
}

/// What the tool will do about `FLASH_OPTR`, and what it refuses to accept.
///
/// The mask exists so the tool only ever writes bits it understands: everything outside it is
/// preserved from whatever the part already had, including reserved bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OptionBytePolicy {
    /// The golden value, as read from a known-good board.
    pub optr: u32,
    /// Which bits of it we own.
    pub optr_mask: u32,
    /// Program only when the masked bits differ. Option flash has finite endurance and there is
    /// no reason to spend it re-writing the value that is already there.
    pub program_if_differs: bool,
}

impl Default for OptionBytePolicy {
    /// ST's factory default for a G0, masked to the bits that matter to this product.
    fn default() -> Self {
        Self {
            optr: 0xFFFF_FEAA,
            optr_mask: Self::DEFAULT_MASK,
            program_if_differs: true,
        }
    }
}

impl OptionBytePolicy {
    /// The bits this tool claims: watchdog selection, boot selection, and reset-pin mode.
    ///
    /// Everything else — RDP, WRP, BOR levels, SRAM parity — is left exactly as found. RDP in
    /// particular is never written by the mask; see [`OptionByteFault::RdpInMask`].
    pub const DEFAULT_MASK: u32 = bits::OPTR_IWDG_SW
        | bits::OPTR_NBOOT_SEL
        | bits::OPTR_NBOOT1
        | bits::OPTR_NBOOT0
        | bits::OPTR_NRST_MODE_MASK;

    /// The value to write, given what the part currently holds.
    pub fn desired(&self, current: u32) -> u32 {
        // Preserve RDP unconditionally, whatever the mask says. Writing it is a different and
        // much more consequential operation than setting a boot bit.
        let merged = (current & !self.optr_mask) | (self.optr & self.optr_mask);
        (merged & !bits::OPTR_RDP_MASK) | (current & bits::OPTR_RDP_MASK)
    }

    pub fn needs_programming(&self, current: u32) -> bool {
        if !self.program_if_differs {
            return false;
        }
        self.desired(current) != current
    }

    /// Reasons to refuse a golden value outright, checked before anything is written.
    pub fn faults(&self) -> Vec<OptionByteFault> {
        let mut faults = Vec::new();

        if self.optr_mask & bits::OPTR_RDP_MASK != 0 {
            faults.push(OptionByteFault::RdpInMask);
        }
        // PA14 is SWCLK *and* BOOT0. With nBOOT_SEL == 0 the part takes BOOT0 from the pin at
        // the rising edge of reset -- and the probe is driving that pin when connect-under-reset
        // releases NRST, at a level nothing here controls. The board can then come out of reset
        // into the system ROM bootloader instead of ours. Flashing still works either way, so
        // the symptom is an intermittently meaningless run-check rather than an obvious failure.
        if self.optr_mask & bits::OPTR_NBOOT_SEL != 0 && self.optr & bits::OPTR_NBOOT_SEL == 0 {
            faults.push(OptionByteFault::BootFromPin);
        }
        // A NRST_MODE that puts PF2 into GPIO silently degrades connect-under-reset into
        // attaching to a *running* target with a live watchdog, and then erasing it.
        if self.optr_mask & bits::OPTR_NRST_MODE_MASK != 0 {
            let mode = (self.optr & bits::OPTR_NRST_MODE_MASK) >> bits::OPTR_NRST_MODE_SHIFT;
            if mode == 0b10 {
                faults.push(OptionByteFault::ResetPinDisabled);
            }
        }
        faults
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptionByteFault {
    /// The mask includes the RDP field. Changing readout protection is a mass-erase-scale
    /// operation and must never ride along with a routine option-byte write.
    RdpInMask,
    /// `nBOOT_SEL == 0`: boot mode would be taken from PA14, which the probe drives as SWCLK.
    BootFromPin,
    /// `NRST_MODE` puts the reset pin into GPIO, so connect-under-reset would not reset.
    ResetPinDisabled,
}

impl core::fmt::Display for OptionByteFault {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            OptionByteFault::RdpInMask => {
                "option mask covers the RDP field; readout protection must never be written by a \
                 routine pass"
            }
            OptionByteFault::BootFromPin => {
                "nBOOT_SEL is 0, so BOOT0 comes from PA14 -- the pin the probe drives as SWCLK"
            }
            OptionByteFault::ResetPinDisabled => {
                "NRST_MODE disables the reset pin, so connect-under-reset would not reset"
            }
        })
    }
}

/// How a pass proves the application is actually executing.
///
/// See the module docs on the run-check for why this is a counter in the main loop rather than
/// the program counter: ARMv6-M cannot read core registers without halting, and halting is
/// exactly what a run-check must not do.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunCheckSpec {
    /// `SCB->VTOR` must read this. Proves the bootloader handed over, and handed over to *us*
    /// rather than to the system ROM.
    ///
    /// It is the base the bundle's application was linked for, not a constant: on a board whose
    /// bootloader started a legacy-base image this reads `0x08006000`, and a check that demanded
    /// `0x08004000` would fail a board that is working perfectly. [`RunCheckSpec::for_base`] is how
    /// a caller says which.
    pub vtor: u32,
    /// Address of a `volatile uint32_t` the application increments in its main loop. Resolved
    /// from the application ELF when the bundle is built, and bound to the image hash below, so
    /// an address/image mismatch cannot happen silently.
    pub liveness_address: u32,
    /// The symbol it came from, for the log.
    pub liveness_symbol: String,
    /// How long to wait between the two samples.
    pub window_ms: u64,
}

impl Default for RunCheckSpec {
    /// The v6 arrangement. Anything flashing a legacy-base image must say so with
    /// [`RunCheckSpec::for_base`]; `Discovery::load` does, from the image's own descriptor.
    fn default() -> Self {
        Self::for_base(addr::APP_BASE)
    }
}

impl RunCheckSpec {
    /// A spec for an application linked at `base`, with no liveness address resolved yet.
    pub fn for_base(base: u32) -> Self {
        Self {
            vtor: base,
            liveness_address: 0,
            liveness_symbol: String::new(),
            window_ms: 200,
        }
    }
}

/// Where a bundle came from. Recorded so a board can be traced back to a build.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum Provenance {
    /// Built from this repository.
    Built {
        /// `PORTAL_GIT_COMMIT` as stamped by `set_build_date.py`.
        git_commit: String,
        git_dirty: bool,
        pio_env: String,
    },
    /// Read off a known-good board over SWD. The path that got bootloaders onto boards before
    /// the bootloader was in version control, and still the fallback until it is.
    Pulled {
        /// Only recorded, never used to make a decision.
        uid: String,
        probe: String,
    },
    /// Two regions from different places — a built application beside a reference bootloader,
    /// say. Each string names where that half came from.
    ///
    /// The alternative was one provenance for a bundle whose halves genuinely have two, which
    /// would have been a comfortable lie in the one record whose whole job is saying where the
    /// bytes came from.
    Composed {
        bootloader: String,
        application: String,
    },
    /// Fabricated. Tests and dry runs only.
    Synthetic,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegionManifest {
    pub name: RegionName,
    pub file: String,
    pub load_address: u32,
    pub bytes: usize,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub v: u32,
    pub kind: String,
    pub probe_rs_target: String,
    pub regions: Vec<RegionManifest>,
    pub option_bytes: OptionBytePolicy,
    pub run_check: RunCheckSpec,
    pub provenance: Provenance,
    /// What this pass does about a bank with no bytes. Part of the manifest, and therefore part of
    /// [`ImageBundle::sha256`], because "the bootloader, and the application left alone" and "the
    /// bootloader, and the application erased" are two different things to do to a board.
    #[serde(default)]
    pub unselected: Unselected,
}

impl Manifest {
    pub const KIND: &'static str = "kc79.portal.swd-image";
    pub const TARGET: &'static str = "STM32G070RBTx";
}

/// A complete, validated device image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageBundle {
    pub bootloader: Region,
    pub application: Region,
    pub option_bytes: OptionBytePolicy,
    pub run_check: RunCheckSpec,
    pub provenance: Provenance,
    /// What this pass does about a bank with no bytes in it.
    ///
    /// Only ever consulted when a bank *is* empty: with both banks supplied the two policies
    /// produce byte-identical work, which is what keeps the production pass out of the blast
    /// radius of this whole feature.
    pub unselected: Unselected,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BundleFault {
    /// A region is not where the firmware build says it should be.
    WrongLoadAddress {
        region: RegionName,
        expected: u32,
        found: u32,
    },
    /// The application is loaded somewhere that is neither application base. There are exactly
    /// two, and a third would mean nothing on the board knows how to start it.
    UnknownApplicationBase {
        found: u32,
    },
    /// The bootloader is larger than the bank it claims.
    BootloaderTooLarge {
        bytes: usize,
        limit: usize,
    },
    /// The bootloader's own pages reach into the application this bundle is programming beside it.
    ///
    /// The brick: a 24 kB (v4/v5) bootloader paired with an application based at `0x08004000`
    /// writes over that application's vector table with its own tail, so the bootloader starts and
    /// then hands over to nothing. Distinct from [`BootloaderTooLarge`] because the bootloader is
    /// not too large for *itself* — it is the pairing that is wrong, and either half could be the
    /// one the operator meant to change.
    ///
    /// [`BootloaderTooLarge`]: BundleFault::BootloaderTooLarge
    BootloaderOverlapsApplication {
        /// One past the last page the bootloader occupies.
        bootloader_end: u32,
        app_base: u32,
    },
    /// The application would run into the durable pages.
    ApplicationTooLarge {
        bytes: usize,
        limit: usize,
    },
    /// Neither region has any bytes. One empty region is a scope; two is a mistake.
    NothingToFlash,
    /// The application's reset vector does not point into the bank it is being loaded into, with
    /// the Thumb bit set. A cheap way to catch an image linked for `0x08000000` being handed to the
    /// application slot -- which would flash cleanly and never run.
    BadResetVector {
        found: u32,
        /// The base it was checked against, since there are two it could have been linked for.
        base: u32,
    },
    /// A legacy-base application on a v6 bootloader. A warning, never a fault: the v6 bootloader
    /// starts an image at `0x08006000` when the new bank is blank, which is exactly the transition
    /// state a fielded board passes through.
    LegacyApplicationOnNewBootloader,
    /// The application is too short to have a vector table at all.
    NoVectorTable,
    OptionBytes(OptionByteFault),
    /// A run-check spec that cannot prove anything.
    NoLivenessAddress,
    /// The liveness address is not in RAM.
    LivenessNotInRam {
        found: u32,
    },
}

impl core::fmt::Display for BundleFault {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            BundleFault::WrongLoadAddress {
                region,
                expected,
                found,
            } => write!(
                f,
                "{} is loaded at {found:#010X}, but the firmware build links it at {expected:#010X}",
                region.as_str()
            ),
            BundleFault::UnknownApplicationBase { found } => write!(
                f,
                "application is loaded at {found:#010X}, which is neither {:#010X} (bootloader v6) \
                 nor {:#010X} (bootloader v4/v5)",
                addr::APP_BASE,
                addr::APP_BASE_LEGACY
            ),
            BundleFault::BootloaderTooLarge { bytes, limit } => write!(
                f,
                "bootloader is {bytes} bytes and would run past its {limit}-byte bank"
            ),
            BundleFault::BootloaderOverlapsApplication {
                bootloader_end,
                app_base,
            } => write!(
                f,
                "the bootloader occupies flash up to {bootloader_end:#010X}, which is past the \
                 application at {app_base:#010X} -- programming this pair would overwrite the \
                 application's vector table and leave a board that cannot be recovered over RS485"
            ),
            BundleFault::ApplicationTooLarge { bytes, limit } => {
                write!(f, "application is {bytes} bytes; the bank holds {limit}")
            }
            BundleFault::NothingToFlash => f.write_str("neither region has any bytes"),
            BundleFault::BadResetVector { found, base } => write!(
                f,
                "application reset vector is {found:#010X}, which is not a Thumb address inside \
                 the application bank at {base:#010X} -- this image is probably linked for \
                 {:#010X}, or for the other application base",
                addr::FLASH_BASE
            ),
            BundleFault::LegacyApplicationOnNewBootloader => write!(
                f,
                "the application is linked for {:#010X} but the bootloader is v6, which would \
                 start an image at {:#010X} -- legal, since v6 falls back to the legacy base, but \
                 it wastes the 8 kB the smaller bootloader freed",
                addr::APP_BASE_LEGACY,
                addr::APP_BASE
            ),
            BundleFault::NoVectorTable => {
                f.write_str("application is too short to contain a vector table")
            }
            BundleFault::OptionBytes(fault) => write!(f, "option bytes: {fault}"),
            BundleFault::NoLivenessAddress => f.write_str(
                "run-check has no liveness address, so it could only prove the bootloader jumped, \
                 not that anything is running",
            ),
            BundleFault::LivenessNotInRam { found } => {
                write!(f, "liveness address {found:#010X} is not in RAM")
            }
        }
    }
}

impl ImageBundle {
    /// Everything that must be true before a byte is written to a board.
    ///
    /// This is the one place the memory map is checked against itself. It runs when a bundle is
    /// loaded, not when a pass starts, so a wrong image is refused at the bench rather than
    /// discovered on a board.
    pub fn validate(&self) -> Vec<BundleFault> {
        let mut faults = Vec::new();

        if self.bootloader.load_address != addr::FLASH_BASE {
            faults.push(BundleFault::WrongLoadAddress {
                region: RegionName::Bootloader,
                expected: addr::FLASH_BASE,
                found: self.bootloader.load_address,
            });
        }
        // Two bases are legal and a third is not. Which of the two is a property of the image,
        // read from its descriptor when it was discovered -- never chosen here.
        let app_base = self.application.load_address;
        if !addr::is_app_base(app_base) {
            faults.push(BundleFault::UnknownApplicationBase { found: app_base });
        }

        // An empty region is a legitimate choice — flashing only the application leaves the
        // bootloader bank erased, which is exactly what bounded firmware-page erasure does. Only a bundle
        // with *nothing* in it is invalid, and reporting per-region emptiness as a fault meant
        // every caller had to know to filter it back out.
        if self.bootloader.bytes.is_empty() && self.application.bytes.is_empty() {
            faults.push(BundleFault::NothingToFlash);
        }

        // A v6 bootloader is held to 16 kB and anything older to 24 kB, because that is what each
        // one's linker script and size gate allow. The version comes out of the image's own banner
        // rather than from the operator: an oversized v6 build is a build mistake, and an
        // unrecognised banner must not silently relax the check to the smaller bank.
        let boot_limit = match crate::device::bootloader_version(&self.bootloader.bytes) {
            Some(version) if version >= 6 => addr::BOOTLOADER_BYTES,
            _ => addr::BOOTLOADER_BYTES_LEGACY,
        } as usize;
        if self.bootloader.bytes.len() > boot_limit {
            faults.push(BundleFault::BootloaderTooLarge {
                bytes: self.bootloader.bytes.len(),
                limit: boot_limit,
            });
        }

        // The pair rule, and the reason this whole check runs before a byte is written.
        //
        // The bootloader is erased and programmed a page at a time, so it occupies whole pages
        // whatever its length. If the last of those pages reaches the application's base, the two
        // images this bundle is programming *together* overlap: the bootloader's tail lands on the
        // application's vector table. The board comes back with a bootloader that starts, an
        // application that does not, and no RS485 path to repair it -- so the pairing is refused
        // here rather than diagnosed afterwards. Only when both halves are actually being written:
        // a bootloader-only pass says nothing about where the application on the board is.
        if !self.bootloader.bytes.is_empty() && !self.application.bytes.is_empty() {
            let pages = self
                .bootloader
                .bytes
                .len()
                .div_ceil(addr::FLASH_PAGE_BYTES as usize);
            let bootloader_end = addr::FLASH_BASE + (pages as u32) * addr::FLASH_PAGE_BYTES;
            if bootloader_end > app_base {
                faults.push(BundleFault::BootloaderOverlapsApplication {
                    bootloader_end,
                    app_base,
                });
            }
        }

        // What fits above the application's own base, which is 8 kB more for a v6 image than for a
        // legacy one. Bounded by the durable pages, never by the end of flash.
        let app_limit = addr::app_bank_bytes(app_base) as usize;
        if self.application.bytes.len() > app_limit {
            faults.push(BundleFault::ApplicationTooLarge {
                bytes: self.application.bytes.len(),
                limit: app_limit,
            });
        }

        // Vector table: [0] initial SP, [1] reset vector -- and only when there is an application
        // to have one. A bootloader-only pass supplies no application bytes on purpose, and a
        // bank that was left out has no vector table to be wrong about.
        //
        // This used to be unconditional, and `Discovery::load` filtered the resulting faults back
        // out for exactly this case (`NoVectorTable`, `BadResetVector`). `ProbeRsRig::flash`
        // re-validates before it touches the probe and did *not* filter, so the two disagreed and
        // a bootloader-only bundle that loaded cleanly was refused at the rig. One check, in one
        // place, with the condition it actually meant.
        if !self.application.bytes.is_empty() {
            if self.application.bytes.len() < 8 {
                faults.push(BundleFault::NoVectorTable);
            } else {
                let reset = u32::from_le_bytes([
                    self.application.bytes[4],
                    self.application.bytes[5],
                    self.application.bytes[6],
                    self.application.bytes[7],
                ]);
                let target = reset & !1;
                let thumb = reset & 1 == 1;
                // Against the base this image is being loaded at, not against the lower of the
                // two: a legacy-base image whose entry point sits below 0x08006000 would be as
                // broken as one linked at 0x08000000, and checking it against 0x08004000 would
                // let it through.
                if !thumb || !(app_base..addr::PERSIST_BASE).contains(&target) {
                    faults.push(BundleFault::BadResetVector {
                        found: reset,
                        base: app_base,
                    });
                }
            }
        }

        for fault in self.option_bytes.faults() {
            faults.push(BundleFault::OptionBytes(fault));
        }

        // An address that exists but is nonsense is a fault. An *absent* one is not: it only
        // stops the automatic run-check, and refusing to flash a perfectly good image because
        // nothing has resolved a symbol out of its ELF yet would be the tail wagging the dog.
        // See `warnings`.
        if self.run_check.liveness_address != 0
            && !(addr::RAM_BASE..addr::RAM_END).contains(&self.run_check.liveness_address)
        {
            faults.push(BundleFault::LivenessNotInRam {
                found: self.run_check.liveness_address,
            });
        }

        faults
    }

    /// Things worth saying about a bundle that do not stop it being flashed.
    ///
    /// Split from [`validate`](Self::validate) because callers treat a fault as a refusal, and
    /// "this can be programmed but not automatically run-checked" is not a refusal.
    pub fn warnings(&self) -> Vec<BundleFault> {
        let mut warnings = Vec::new();
        if self.run_check.liveness_address == 0 {
            warnings.push(BundleFault::NoLivenessAddress);
        }
        // Wasteful rather than wrong, so it is said and not refused. It is also the state a board
        // is deliberately left in halfway through a bootloader replacement: new bootloader first,
        // rebased application afterwards.
        if !self.application.bytes.is_empty()
            && self.application.load_address == addr::APP_BASE_LEGACY
            && crate::device::bootloader_version(&self.bootloader.bytes).is_some_and(|v| v >= 6)
        {
            warnings.push(BundleFault::LegacyApplicationOnNewBootloader);
        }
        warnings
    }

    pub fn manifest(&self) -> Manifest {
        Manifest {
            v: 1,
            kind: Manifest::KIND.to_owned(),
            probe_rs_target: Manifest::TARGET.to_owned(),
            regions: vec![
                region_manifest(&self.bootloader, "bootloader.bin"),
                region_manifest(&self.application, "application.bin"),
            ],
            option_bytes: self.option_bytes,
            run_check: self.run_check.clone(),
            provenance: self.provenance.clone(),
            unselected: self.unselected,
        }
    }

    /// One hash identifying the whole bundle: the manifest as canonical JSON, then each region's
    /// bytes in order. This is what a log records, and what the run-check spec is bound to.
    pub fn sha256(&self) -> String {
        let mut hasher = Sha256::new();
        let manifest = serde_json::to_vec(&self.manifest())
            .expect("the manifest is plain data and cannot fail");
        hasher.update(&manifest);
        hasher.update(&self.bootloader.bytes);
        hasher.update(&self.application.bytes);
        hex(&hasher.finalize())
    }

    /// Which banks this pass actually writes, in the words the page uses.
    pub fn scope(&self) -> &'static str {
        match (
            !self.bootloader.bytes.is_empty(),
            !self.application.bytes.is_empty(),
        ) {
            (true, true) => "full",
            (true, false) => "bootloader only",
            (false, true) => "application only",
            (false, false) => "nothing",
        }
    }

    /// The spans this pass erases, programs and reads back.
    ///
    /// Under [`Unselected::Erase`] that is the whole firmware partition in one window, which is
    /// bit-for-bit what every pass staged before this method existed. Under
    /// [`Unselected::Preserve`] it is one window per *supplied* bank, each covering that bank
    /// entirely.
    ///
    /// Covering the whole bank rather than just the image matters: a new bootloader shorter than
    /// the one already on the board must not leave the old one's tail behind, and a shorter
    /// application must not leave a stale tail that `Layout` would still read as a valid image.
    /// So each window is the image padded with `0xFF` to the bank end -- which is also what makes
    /// every window page-aligned at both ends, since the bank boundaries are.
    pub fn write_windows(&self) -> Vec<Window> {
        if self.unselected == Unselected::Erase {
            return vec![Window {
                start: addr::FLASH_BASE,
                bytes: self.expected_firmware_image(),
            }];
        }
        self.banks()
            .into_iter()
            .filter(|(region, _, _)| !region.bytes.is_empty())
            .map(|(region, start, end)| {
                let mut bytes = vec![0xFF_u8; (end - start) as usize];
                bytes[..region.bytes.len()].copy_from_slice(&region.bytes);
                Window { start, bytes }
            })
            .collect()
    }

    /// The two banks this bundle writes into, as `(region, start, end)`.
    ///
    /// The boundary between them is the application's own base, so a legacy-base application gets
    /// the bank it was linked for and the bootloader beside it gets everything below that. Both
    /// ends of both banks stay page-aligned, because both bases are.
    ///
    /// With no application in the pass there is no boundary to take from it, and the bootloader's
    /// bank falls back to whichever of the two banks its own image needs. Erasing further would
    /// mean erasing pages 8-11 -- which is where a v6 board's application starts, and this pass
    /// knows nothing about what is on the board.
    fn banks(&self) -> [(&Region, u32, u32); 2] {
        let boundary = if self.application.bytes.is_empty() {
            if self.bootloader.bytes.len() > addr::BOOTLOADER_BYTES as usize {
                addr::APP_BASE_LEGACY
            } else {
                addr::APP_BASE
            }
        } else {
            self.application.load_address
        };
        [
            (&self.bootloader, addr::FLASH_BASE, boundary),
            (&self.application, boundary, addr::PERSIST_BASE),
        ]
    }

    /// The firmware spans this pass must leave exactly as it found them, as `(start, end)`.
    ///
    /// The complement of [`write_windows`](Self::write_windows) within the firmware partition, and
    /// evidence rather than a plan: the rig reads these before programming and compares them
    /// after, the same treatment the durable pages already get. A "preserve" nobody checked is
    /// indistinguishable from a preserve that did not happen.
    ///
    /// Empty under [`Unselected::Erase`], where every firmware page is written by definition.
    pub fn preserved_windows(&self) -> Vec<(u32, u32)> {
        if self.unselected == Unselected::Erase {
            return Vec::new();
        }
        self.banks()
            .into_iter()
            .filter(|(region, _, _)| region.bytes.is_empty())
            .map(|(_, start, end)| (start, end))
            .collect()
    }

    /// The firmware bytes a readback should produce. Durable pages are intentionally absent:
    /// they are validated as records, not compared with an erased-image fiction.
    pub fn expected_firmware_image(&self) -> Vec<u8> {
        let span = addr::FIRMWARE_BYTES as usize;
        let mut image = vec![0xFF_u8; span];
        for region in [&self.bootloader, &self.application] {
            let offset = (region.load_address - addr::FLASH_BASE) as usize;
            let end = offset + region.bytes.len();
            if end <= image.len() {
                image[offset..end].copy_from_slice(&region.bytes);
            }
        }
        image
    }

    /// Compatibility name retained for callers. This now means the firmware partition only.
    pub fn expected_flash_image(&self) -> Vec<u8> {
        self.expected_firmware_image()
    }
}

fn region_manifest(region: &Region, file: &str) -> RegionManifest {
    RegionManifest {
        name: region.name,
        file: file.to_owned(),
        load_address: region.load_address,
        bytes: region.bytes.len(),
        sha256: region.sha256(),
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use core::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
}
