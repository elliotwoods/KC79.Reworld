//! What is actually on a board, worked out from its flash and its option bytes.
//!
//! Pure: everything here is a function of 128 kB of bytes plus four registers, so it is tested
//! against values measured off a real board rather than against values invented to suit it.
//!
//! # Layout is classified, not assumed
//!
//! The design assumed every Portal is bootloader-plus-application. The first board on the bench
//! was not: a single flat image linked at `0x08000000`, 102,396 bytes, whose reset vector points
//! to `0x0800BC55` — past the `0x08006000` bank boundary. That is a `no_bootloader` build, and
//! reporting it as "two broken regions" would be worse than saying nothing. So [`Layout`] is
//! something this module works out and the UI displays, never something the caller presumes.
//!
//! # Identification needs no symbols
//!
//! `PORTAL_VERSION_STRING` and the bootloader's `Bootloader v4` are plain string literals, so
//! they survive into the binary and can be read straight out of a readback. Verified on the
//! reference bootloader, three HomeSwitchTest builds, and a live board reading
//! `Portal v2026-08-10_15.01`. No ELF, no symbol table, no firmware change.

use sha2::{Digest, Sha256};

use crate::addr;
use crate::bits;
use crate::persistent::{IdentityState, McuUid, SettingsState};

/// A raw readback plus the registers worth capturing with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceImage {
    /// The whole 128 kB, `0x08000000..0x08020000`.
    pub flash: Vec<u8>,
    pub optr: u32,
    /// `DBGMCU_IDCODE`. Readable on the non-invasive path in practice, despite
    /// `RCC_APBENR1.DBGEN` reading 0 — measured, so it is `Option` rather than assumed either way.
    pub idcode: Option<u32>,
    pub uid: [u32; 3],
    pub flash_kb: u16,
    pub rcc_csr: u32,
}

/// The initial stack pointer and reset vector at the head of a region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VectorTable {
    pub initial_sp: u32,
    pub reset: u32,
}

impl VectorTable {
    /// Read a vector table, if the eight bytes at `offset` plausibly are one.
    ///
    /// The initial stack pointer legitimately points **one past** the top of RAM, because the
    /// stack grows down — `0x20009000` on a 36 kB part is correct. An exclusive upper bound here
    /// reports every real image as broken, which is exactly what the first version did.
    pub fn read(flash: &[u8], offset: usize) -> Option<Self> {
        if flash.len() < offset + 8 {
            return None;
        }
        let word = |at: usize| {
            u32::from_le_bytes([flash[at], flash[at + 1], flash[at + 2], flash[at + 3]])
        };
        let initial_sp = word(offset);
        let reset = word(offset + 4);

        let sp_ok = (addr::RAM_BASE..=addr::RAM_END).contains(&initial_sp);
        let pc_ok = reset & 1 == 1 && (addr::FLASH_BASE..addr::FLASH_END).contains(&(reset & !1));
        (sp_ok && pc_ok).then_some(Self { initial_sp, reset })
    }

    /// Where execution actually starts, with the Thumb bit removed.
    pub fn entry(&self) -> u32 {
        self.reset & !1
    }
}

/// How the flash is arranged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// Nothing programmed anywhere.
    Erased,
    /// Bootloader at `0x08000000` and application at `0x08006000` — the production arrangement.
    Split,
    /// One image linked at `0x08000000` spanning past the bank boundary. A `no_bootloader` build:
    /// it runs, but the RS485 field-update path cannot touch it, because that path needs the
    /// bootloader.
    Flat,
    /// Something is programmed but no valid vector table was found where one should be.
    Unrecognised,
}

impl Layout {
    pub fn as_str(self) -> &'static str {
        match self {
            Layout::Erased => "erased",
            Layout::Split => "split",
            Layout::Flat => "flat",
            Layout::Unrecognised => "unrecognised",
        }
    }

    /// Whether this arrangement can be field-updated over RS485. Only a real bootloader can.
    pub fn supports_field_update(self) -> bool {
        self == Layout::Split
    }
}

/// One bank's worth of findings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegionReport {
    pub name: &'static str,
    pub base: u32,
    /// Bytes up to and including the last non-erased one.
    pub used_bytes: usize,
    pub vector: Option<VectorTable>,
    /// A version banner scraped out of the bytes, if there is one.
    pub banner: Option<String>,
    /// Of the used bytes only, so it can be compared with a bundle region's hash.
    pub sha256: String,
}

impl RegionReport {
    pub fn is_erased(&self) -> bool {
        self.used_bytes == 0
    }
}

/// `FLASH_OPTR`, decoded into the fields that decide whether a board is safe to work on.
///
/// **The bit positions for `nBOOT_SEL`, `nBOOT0`, `nBOOT1` and `NRST_MODE` come from libopencm3,
/// not from ST.** They are consistent with a board measured at `0xDFFFE1AA` reading as a sane
/// production configuration, which is corroboration rather than proof. Check RM0454 §3.4.1 before
/// anything *writes* an option byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OptionBytes {
    pub raw: u32,
    pub rdp: u8,
    /// True when the watchdog is selected by software. False means hardware starts it at every
    /// reset regardless of firmware, which promotes the debug freeze from precaution to necessity.
    pub iwdg_sw: bool,
    /// True when boot mode comes from the option bits. **False means it comes from the
    /// PA14-BOOT0 pin — the pin the probe drives as SWCLK.**
    pub nboot_sel: bool,
    pub nboot0: bool,
    pub nboot1: bool,
    pub nrst_mode: u8,
}

impl OptionBytes {
    pub fn decode(raw: u32) -> Self {
        Self {
            raw,
            rdp: (raw & bits::OPTR_RDP_MASK) as u8,
            iwdg_sw: raw & bits::OPTR_IWDG_SW != 0,
            nboot_sel: raw & bits::OPTR_NBOOT_SEL != 0,
            nboot0: raw & bits::OPTR_NBOOT0 != 0,
            nboot1: raw & bits::OPTR_NBOOT1 != 0,
            nrst_mode: ((raw & bits::OPTR_NRST_MODE_MASK) >> bits::OPTR_NRST_MODE_SHIFT) as u8,
        }
    }

    /// Readout protection level, as ST numbers them.
    pub fn rdp_level(&self) -> u8 {
        match u32::from(self.rdp) {
            bits::OPTR_RDP_LEVEL0 => 0,
            bits::OPTR_RDP_LEVEL2 => 2,
            _ => 1,
        }
    }

    /// Everything about this configuration that would make a pass behave badly.
    pub fn warnings(&self) -> Vec<OptionWarning> {
        let mut out = Vec::new();
        match self.rdp_level() {
            0 => {}
            2 => out.push(OptionWarning::ReadoutProtectedPermanently),
            _ => out.push(OptionWarning::ReadoutProtected),
        }
        if !self.nboot_sel {
            out.push(OptionWarning::BootFromPin);
        }
        // 0b10 is GPIO on this family; the other encodings keep NRST working as a reset.
        if self.nrst_mode == 0b10 {
            out.push(OptionWarning::ResetPinDisabled);
        }
        if !self.iwdg_sw {
            out.push(OptionWarning::HardwareWatchdog);
        }
        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptionWarning {
    ReadoutProtected,
    ReadoutProtectedPermanently,
    BootFromPin,
    ResetPinDisabled,
    HardwareWatchdog,
}

impl core::fmt::Display for OptionWarning {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            OptionWarning::ReadoutProtected => {
                "readout protection is on; recovering means a mass erase"
            }
            OptionWarning::ReadoutProtectedPermanently => {
                "RDP level 2 -- debug is permanently disabled on this part"
            }
            OptionWarning::BootFromPin => {
                "nBOOT_SEL is 0, so BOOT0 comes from PA14 -- the pin the probe drives as SWCLK, \
                 which can leave the part in the system ROM after connect-under-reset"
            }
            OptionWarning::ResetPinDisabled => {
                "NRST is configured as GPIO, so connect-under-reset will not actually reset"
            }
            OptionWarning::HardwareWatchdog => {
                "IWDG_SW is 0: the watchdog starts in hardware at every reset, so the debug \
                 freeze is required rather than precautionary"
            }
        })
    }
}

/// Everything worth saying about a board.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceReport {
    pub layout: Layout,
    pub bootloader: RegionReport,
    pub application: RegionReport,
    /// The whole-flash vector table, meaningful when the layout is [`Layout::Flat`].
    pub flat_vector: Option<VectorTable>,
    pub options: OptionBytes,
    pub uid: String,
    pub idcode: Option<u32>,
    pub dev_id: Option<u16>,
    pub flash_kb: u16,
    pub programmed_bytes: usize,
    pub total_bytes: usize,
    pub identity: IdentityState,
    pub settings: SettingsState,
}

impl DeviceImage {
    pub fn analyse(&self) -> DeviceReport {
        let split = addr::BOOTLOADER_BYTES as usize;
        let firmware_end = addr::FIRMWARE_BYTES as usize;
        let firmware = &self.flash[..firmware_end.min(self.flash.len())];
        let boot_vector = VectorTable::read(&self.flash, 0);
        let app_vector = VectorTable::read(&self.flash, split);

        let boot_used = used_bytes(&firmware[..split.min(firmware.len())]);
        let app_bytes = &firmware[split.min(firmware.len())..];
        let app_used = used_bytes(app_bytes);

        // A vector table at 0 whose entry point is past the bank boundary means one image spans
        // both banks -- there is no bootloader, whatever else is programmed.
        let flat = boot_vector.is_some_and(|v| v.entry() >= addr::APP_BASE);

        let layout = if boot_used == 0 && app_used == 0 {
            Layout::Erased
        } else if flat {
            Layout::Flat
        } else if boot_vector.is_some() && app_vector.is_some() {
            Layout::Split
        } else {
            Layout::Unrecognised
        };

        DeviceReport {
            layout,
            bootloader: RegionReport {
                name: "bootloader",
                base: addr::FLASH_BASE,
                used_bytes: boot_used,
                vector: if flat { None } else { boot_vector },
                banner: first_banner(&self.flash[..split.min(self.flash.len())]),
                sha256: hex(&Sha256::digest(&self.flash[..boot_used])),
            },
            application: RegionReport {
                name: "application",
                base: addr::APP_BASE,
                used_bytes: app_used,
                vector: app_vector,
                banner: first_banner(app_bytes),
                sha256: hex(&Sha256::digest(&app_bytes[..app_used])),
            },
            flat_vector: if flat { boot_vector } else { None },
            options: OptionBytes::decode(self.optr),
            uid: format!(
                "{:08X}-{:08X}-{:08X}",
                self.uid[0], self.uid[1], self.uid[2]
            ),
            idcode: self.idcode,
            dev_id: self.idcode.map(|v| (v & 0xFFF) as u16),
            flash_kb: self.flash_kb,
            programmed_bytes: self.flash.iter().filter(|&&b| b != 0xFF).count(),
            total_bytes: self.flash.len(),
            identity: {
                let at = (addr::IDENTITY_BASE - addr::FLASH_BASE) as usize;
                let end = at + addr::FLASH_PAGE_BYTES as usize;
                crate::persistent::scan_identity_page(
                    self.flash.get(at..end).unwrap_or(&[]),
                    McuUid(self.uid),
                )
            },
            settings: {
                let a = (addr::SETTINGS_A_BASE - addr::FLASH_BASE) as usize;
                let b = (addr::SETTINGS_B_BASE - addr::FLASH_BASE) as usize;
                let page = addr::FLASH_PAGE_BYTES as usize;
                SettingsState::load(
                    self.flash.get(a..a + page).unwrap_or(&[]),
                    self.flash.get(b..b + page).unwrap_or(&[]),
                    McuUid(self.uid),
                )
            },
        }
    }

    /// Per-bucket programmed fraction, 0..=255, for drawing the map.
    ///
    /// Bucketed rather than per-byte because the map is a few hundred pixels wide and the browser
    /// should not be handed 128 kB to reduce on every render.
    pub fn occupancy(&self, buckets: usize) -> Vec<u8> {
        occupancy_of(&self.flash, buckets)
    }
}

/// Shared with the selected-image lane, so both lanes of the map are computed the same way.
pub fn occupancy_of(bytes: &[u8], buckets: usize) -> Vec<u8> {
    if buckets == 0 || bytes.is_empty() {
        return Vec::new();
    }
    (0..buckets)
        .map(|index| {
            let from = index * bytes.len() / buckets;
            let to = ((index + 1) * bytes.len() / buckets)
                .max(from + 1)
                .min(bytes.len());
            let slice = &bytes[from..to];
            let used = slice.iter().filter(|&&b| b != 0xFF).count();
            ((used * 255) / slice.len().max(1)) as u8
        })
        .collect()
}

fn used_bytes(region: &[u8]) -> usize {
    region.iter().rposition(|&b| b != 0xFF).map_or(0, |i| i + 1)
}

/// The version banners this product puts in its binaries.
const BANNER_NEEDLES: [&str; 2] = ["Portal v", "Bootloader v"];

/// The first printable run starting at a known banner prefix.
pub fn first_banner(region: &[u8]) -> Option<String> {
    BANNER_NEEDLES.iter().find_map(|needle| {
        let needle = needle.as_bytes();
        region
            .windows(needle.len())
            .position(|w| w == needle)
            .map(|start| {
                region[start..]
                    .iter()
                    .take(64)
                    .take_while(|&&b| (0x20..0x7F).contains(&b))
                    .map(|&b| b as char)
                    .collect()
            })
    })
}

/// The hash of a blob of device bytes, in the one format the reports and the log both use.
///
/// Public because a flash pass hashes what it read *back* off the board rather than what it meant
/// to send, and that hash has to be comparable with the per-region ones in a [`DeviceReport`].
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    use core::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read off the board on the bench, 2026-08-17. Every number here is measured.
    const MEASURED_OPTR: u32 = 0xDFFF_E1AA;

    fn blank() -> Vec<u8> {
        vec![0xFF; (addr::FLASH_END - addr::FLASH_BASE) as usize]
    }

    fn image(flash: Vec<u8>) -> DeviceImage {
        DeviceImage {
            flash,
            optr: MEASURED_OPTR,
            idcode: Some(0x2001_6460),
            uid: [0x0028_0055, 0x3035_5110, 0x3332_3636],
            flash_kb: 128,
            rcc_csr: 0x0C00_0000,
        }
    }

    fn put_vector(flash: &mut [u8], offset: usize, sp: u32, reset: u32) {
        flash[offset..offset + 4].copy_from_slice(&sp.to_le_bytes());
        flash[offset + 4..offset + 8].copy_from_slice(&reset.to_le_bytes());
    }

    // ---------------------------------------------------------------- vector tables

    #[test]
    fn the_initial_stack_pointer_may_point_one_past_the_top_of_ram() {
        // The stack grows down, so 0x20009000 on a 36 kB part is correct. An exclusive upper
        // bound here reported every real image as broken -- which is what the first version did,
        // on a board that was working perfectly.
        let mut flash = blank();
        put_vector(&mut flash, 0, 0x2000_9000, 0x0800_123D);
        assert_eq!(
            VectorTable::read(&flash, 0),
            Some(VectorTable {
                initial_sp: 0x2000_9000,
                reset: 0x0800_123D
            })
        );
    }

    #[test]
    fn a_vector_table_needs_the_thumb_bit_and_a_flash_entry_point() {
        let mut flash = blank();
        put_vector(&mut flash, 0, 0x2000_9000, 0x0800_123C); // Thumb bit clear
        assert_eq!(VectorTable::read(&flash, 0), None);

        put_vector(&mut flash, 0, 0x2000_9000, 0x2000_0001); // entry in RAM
        assert_eq!(VectorTable::read(&flash, 0), None);

        put_vector(&mut flash, 0, 0x6929_D003, 0x41B6_42B4); // the garbage a real board showed
        assert_eq!(VectorTable::read(&flash, 0), None);
    }

    // ---------------------------------------------------------------- layout

    #[test]
    fn an_erased_part_reads_as_erased() {
        let report = image(blank()).analyse();
        assert_eq!(report.layout, Layout::Erased);
        assert!(report.bootloader.is_erased());
        assert!(report.application.is_erased());
        assert_eq!(report.programmed_bytes, 0);
    }

    #[test]
    fn the_production_arrangement_reads_as_split() {
        let mut flash = blank();
        // Bootloader: entry inside its own 24 kB bank.
        put_vector(&mut flash, 0, 0x2000_9000, 0x0800_123D);
        flash[0x100] = 0x01;
        // Application: entry inside the application bank.
        put_vector(&mut flash, 0x6000, 0x2000_9000, 0x0800_6241);
        flash[0x6100] = 0x02;

        let report = image(flash).analyse();
        assert_eq!(report.layout, Layout::Split);
        assert!(report.bootloader.vector.is_some());
        assert!(report.application.vector.is_some());
        assert!(report.layout.supports_field_update());
    }

    /// The board actually on the bench. Not a hypothetical.
    #[test]
    fn a_no_bootloader_build_reads_as_flat_not_as_two_broken_regions() {
        let mut flash = blank();
        put_vector(&mut flash, 0, 0x2000_9000, 0x0800_BC55);
        for byte in flash.iter_mut().take(102_396).skip(8) {
            *byte = 0x5A;
        }

        let report = image(flash).analyse();
        assert_eq!(report.layout, Layout::Flat);
        assert_eq!(report.flat_vector.map(|v| v.entry()), Some(0x0800_BC54));
        assert!(
            report.bootloader.vector.is_none(),
            "a flat image must not be reported as a bootloader; the bytes at 0x08000000 are its \
             vector table, not a bootloader's"
        );
        assert!(
            !report.layout.supports_field_update(),
            "a flat image has no bootloader, so RS485 field update cannot reach it"
        );
    }

    #[test]
    fn programmed_but_meaningless_reads_as_unrecognised() {
        let mut flash = blank();
        flash[0x400] = 0x11;
        let report = image(flash).analyse();
        assert_eq!(report.layout, Layout::Unrecognised);
    }

    // ---------------------------------------------------------------- identification

    #[test]
    fn a_version_banner_is_read_straight_out_of_the_bytes() {
        let mut flash = blank();
        put_vector(&mut flash, 0x6000, 0x2000_9000, 0x0800_6241);
        let banner = b"Portal v2026-08-10_15.01\0";
        flash[0x6800..0x6800 + banner.len()].copy_from_slice(banner);

        let report = image(flash).analyse();
        assert_eq!(
            report.application.banner.as_deref(),
            Some("Portal v2026-08-10_15.01"),
            "identifying a board must need no symbols and no firmware change"
        );
    }

    #[test]
    fn a_banner_stops_at_the_first_unprintable_byte() {
        let mut region = vec![0xFF; 256];
        region[10..10 + 12].copy_from_slice(b"Portal v1.2\x00");
        region[22] = b'X';
        assert_eq!(first_banner(&region).as_deref(), Some("Portal v1.2"));
    }

    // ---------------------------------------------------------------- option bytes

    #[test]
    fn the_measured_board_decodes_as_a_safe_configuration() {
        // 0xDFFFE1AA, read off the bench board. This is the corroboration for the libopencm3 bit
        // positions: if any of them were wrong, at least one of these would read absurdly.
        let opts = OptionBytes::decode(MEASURED_OPTR);
        assert_eq!(opts.rdp_level(), 0, "RDP 0xAA is level 0");
        assert!(opts.iwdg_sw, "the watchdog is software-selected");
        assert!(
            opts.nboot_sel,
            "nBOOT_SEL is 1, so BOOT0 comes from the option bits and NOT from PA14/SWCLK -- \
             which is the safe value, and contradicts the guess that fielded units run 0"
        );
        assert_eq!(opts.nrst_mode, 0b11, "NRST is a bidirectional reset");
        assert_eq!(
            opts.warnings(),
            vec![],
            "nothing about this board is unsafe to work on"
        );
    }

    #[test]
    fn boot_from_the_swclk_pin_is_a_warning() {
        let opts = OptionBytes::decode(MEASURED_OPTR & !bits::OPTR_NBOOT_SEL);
        assert!(opts.warnings().contains(&OptionWarning::BootFromPin));
    }

    #[test]
    fn a_disabled_reset_pin_is_a_warning() {
        let raw =
            (MEASURED_OPTR & !bits::OPTR_NRST_MODE_MASK) | (0b10 << bits::OPTR_NRST_MODE_SHIFT);
        assert!(
            OptionBytes::decode(raw)
                .warnings()
                .contains(&OptionWarning::ResetPinDisabled),
            "connect-under-reset silently degrades into erasing a running target"
        );
    }

    #[test]
    fn readout_protection_is_a_warning_and_level_two_is_a_different_one() {
        // RDP byte 0x00 is neither 0xAA nor 0xCC, so it is level 1.
        let one = OptionBytes::decode(MEASURED_OPTR & !bits::OPTR_RDP_MASK);
        assert_eq!(one.rdp_level(), 1);
        assert!(one.warnings().contains(&OptionWarning::ReadoutProtected));

        let two =
            OptionBytes::decode((MEASURED_OPTR & !bits::OPTR_RDP_MASK) | bits::OPTR_RDP_LEVEL2);
        assert_eq!(two.rdp_level(), 2);
        assert!(
            two.warnings()
                .contains(&OptionWarning::ReadoutProtectedPermanently)
        );
    }

    // ---------------------------------------------------------------- the map

    #[test]
    fn occupancy_buckets_span_the_whole_image() {
        let mut flash = blank();
        // Programme exactly the first half.
        for byte in flash.iter_mut().take(64 * 1024) {
            *byte = 0x00;
        }
        let buckets = occupancy_of(&flash, 128);
        assert_eq!(buckets.len(), 128);
        assert!(
            buckets[..64].iter().all(|&b| b == 255),
            "first half fully programmed"
        );
        assert!(buckets[64..].iter().all(|&b| b == 0), "second half erased");
    }

    #[test]
    fn occupancy_is_defined_for_awkward_bucket_counts() {
        let flash = blank();
        assert_eq!(occupancy_of(&flash, 0).len(), 0);
        assert_eq!(occupancy_of(&[], 16).len(), 0);
        // More buckets than bytes must not panic or divide by zero.
        assert_eq!(occupancy_of(&[0x00, 0xFF], 8).len(), 8);
    }
}
