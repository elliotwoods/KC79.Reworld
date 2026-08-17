//! What gets flashed, and everything about it that is not bytes.
//!
//! A bundle is two binaries and a manifest:
//!
//! ```text
//! images/portal-2026-08-17/
//!   manifest.json
//!   bootloader.bin      -> 0x08000000, <= 24 kB
//!   application.bin     -> 0x08006000
//! ```
//!
//! Two binaries rather than one merged blob, because the RS485 field-update path can only ever
//! ship `application.bin`, and it needs it as a bare image starting at offset zero — which is
//! exactly what `FWUpdate::uploadFirmware` and `fw_update::upload` consume. A merged image is
//! useless to that path.
//!
//! The manifest carries the things a bare `.bin` cannot. Chief among them the load address:
//! `0x08006000` is currently a magic number duplicated across `PortalFW/platformio.ini`,
//! `PortalFW/set_bank2.py` and the bootloader's `constants.h`, and nothing compares the three.
//! Here it is data, and [`ImageBundle::validate`] is the thing that compares them.
//!
//! # Both regions, always
//!
//! A virgin part has `0xFFFFFFFF` at `0x08000000`, so its initial stack pointer and reset vector
//! are garbage and the core locks up the instant it leaves reset. Nothing ever reaches the
//! application at `0x08006000`, because the thing that jumps there is the bootloader. So a
//! bundle that could only describe the application would be unable to bring up a new board at
//! all — which is precisely the job.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::addr;
use crate::bits;

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
    fn default() -> Self {
        Self {
            vtor: addr::APP_BASE,
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BundleFault {
    /// A region is not where the firmware build says it should be.
    WrongLoadAddress {
        region: RegionName,
        expected: u32,
        found: u32,
    },
    /// The bootloader would run into the application bank.
    BootloaderTooLarge {
        bytes: usize,
        limit: usize,
    },
    /// The application would run off the end of flash.
    ApplicationTooLarge {
        bytes: usize,
        limit: usize,
    },
    /// Neither region has any bytes. One empty region is a scope; two is a mistake.
    NothingToFlash,
    /// The application's reset vector does not point into the application bank with the Thumb
    /// bit set. A cheap way to catch an image linked for `0x08000000` being handed to the
    /// application slot -- which would flash cleanly and never run.
    BadResetVector {
        found: u32,
    },
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
            BundleFault::BootloaderTooLarge { bytes, limit } => write!(
                f,
                "bootloader is {bytes} bytes and would overlap the application bank at {limit}"
            ),
            BundleFault::ApplicationTooLarge { bytes, limit } => {
                write!(f, "application is {bytes} bytes; the bank holds {limit}")
            }
            BundleFault::NothingToFlash => f.write_str("neither region has any bytes"),
            BundleFault::BadResetVector { found } => write!(
                f,
                "application reset vector is {found:#010X}, which is not a Thumb address inside \
                 the application bank -- this image is probably linked for {:#010X}",
                addr::FLASH_BASE
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
        if self.application.load_address != addr::APP_BASE {
            faults.push(BundleFault::WrongLoadAddress {
                region: RegionName::Application,
                expected: addr::APP_BASE,
                found: self.application.load_address,
            });
        }

        // An empty region is a legitimate choice — flashing only the application leaves the
        // bootloader bank erased, which is exactly what a mass erase then does. Only a bundle
        // with *nothing* in it is invalid, and reporting per-region emptiness as a fault meant
        // every caller had to know to filter it back out.
        if self.bootloader.bytes.is_empty() && self.application.bytes.is_empty() {
            faults.push(BundleFault::NothingToFlash);
        }

        let boot_limit = addr::BOOTLOADER_BYTES as usize;
        if self.bootloader.bytes.len() > boot_limit {
            faults.push(BundleFault::BootloaderTooLarge {
                bytes: self.bootloader.bytes.len(),
                limit: boot_limit,
            });
        }
        let app_limit = addr::APP_BANK_BYTES as usize;
        if self.application.bytes.len() > app_limit {
            faults.push(BundleFault::ApplicationTooLarge {
                bytes: self.application.bytes.len(),
                limit: app_limit,
            });
        }

        // Vector table: [0] initial SP, [1] reset vector.
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
            if !thumb || !(addr::APP_BASE..addr::FLASH_END).contains(&target) {
                faults.push(BundleFault::BadResetVector { found: reset });
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

    /// The bytes a full-device readback should produce, with erased flash where nothing is
    /// programmed. Readback verification compares against exactly this.
    pub fn expected_flash_image(&self) -> Vec<u8> {
        let span = (addr::FLASH_END - addr::FLASH_BASE) as usize;
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
