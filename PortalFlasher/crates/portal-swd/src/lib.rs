//! SWD flashing of KC79 Portal boards (STM32G070RBT6).
//!
//! The crate is split so that the part worth being certain about can be tested without a probe,
//! a board, or a GUI:
//!
//! - [`machine`] is the rig's policy — arm, debounce, flash, cycle, run-check, removal gate. It
//!   is pure and clock-injected, so a bouncing hand-plug is a unit test.
//! - the probe, the image bundle and the pass implementations sit behind traits, so the same
//!   policy runs against real hardware or against a modelled target with fault injection.
//!
//! Nothing here depends on `av-*`. The operator application drives this crate; this crate knows
//! nothing about the application.
//!
//! # The device
//!
//! | | |
//! |---|---|
//! | Part | STM32G070RBT6, Cortex-M0+, 128 kB flash, 36 kB RAM |
//! | Bootloader | `0x08000000`, 16 kB (v6) or 24 kB (v4/v5) — RS485 field updater, and what starts the IWDG |
//! | Application | `0x08004000` (v6) or `0x08006000` (v4/v5) — built by `PortalFW`, sets `VTOR` to its own base |
//! | Debug | SWD on PA13/PA14. **PA14 is also BOOT0**, which is why `nBOOT_SEL` matters |
//!
//! # Two application bases
//!
//! The v6 bootloader is 16 kB where v4 and v5 were 24 kB, so the application base moved down to
//! [`addr::APP_BASE`]. Both bases are live and neither is deprecated: a board still carrying a v4
//! or v5 bootloader runs its application at [`addr::APP_BASE_LEGACY`], and a v6 bootloader starts
//! an image there too when the new bank is blank — which is what makes replacing a fielded
//! bootloader a survivable single step rather than a flag day.
//!
//! So "where does the application live" is a property of the *target* (read from the board, see
//! [`device::DeviceReport`]) and "where was this image linked" is a property of the *image* (read
//! from its descriptor, see [`image::image_base`]). Neither is a constant, and the one place they
//! meet is [`image::ImageBundle::validate`].

pub mod artefacts;
pub mod device;
pub mod elf;
pub mod image;
pub mod machine;
pub mod persistent;
pub mod program;
pub mod rig;
pub mod staging;
pub mod symbols;

#[cfg(feature = "probe")]
pub mod probe;

pub use artefacts::{Artefact, Discovery, Origin, discover};
pub use device::{DeviceImage, DeviceReport, Layout, OptionBytes, OptionWarning, VectorTable};

pub use image::{
    AppDescriptor, BaseSource, BundleFault, ImageBundle, OptionBytePolicy, Region, RegionName,
    RunCheckSpec, Unselected, Window, image_base, read_descriptor,
};
pub use machine::{Action, Cue, Input, Machine, Millis, Pass, Phase, Sequence, Timing};
pub use persistent::{
    DeviceSettings, IdentityRecord, IdentityState, JournalWrite, OpticalCalibration,
    SettingsRecord, SettingsSource,
};
#[cfg(feature = "probe")]
pub use probe::{ProbeDescriptor, ProbeRsRig, list_probes};
pub use rig::{
    BootFault, BootReport, FlashReport, PersistentWriteReport, Presence, ProbeInfo, Release, Rig,
    RigError, RigErrorKind, RunCheckFault, RunCheckReport, SimRig, Step, Trigger,
};

/// Fixed addresses on the STM32G070, verified against the CMSIS device header
/// (`framework-arduinoststm32/system/Drivers/CMSIS/Device/ST/STM32G0xx/Include/stm32g070xx.h`)
/// rather than recalled.
///
/// The flash and RAM map is mirrored from `PortalBootloader/include/portal_flash_layout.h`, which
/// is the definition; `the_memory_map_agrees_with_the_firmware_header` reads that file as text and
/// asserts every constant here against it, so a change on the firmware side that nobody mirrored
/// fails `cargo test` rather than corrupting a board.
///
/// The `FLASH_OPTR` *field* positions are the exception: those come from libopencm3, not from
/// ST, and must be checked against RM0454 §3.4.1 before anything writes an option byte. Getting
/// `nBOOT_SEL` wrong changes how the part boots.
pub mod addr {
    /// Start of flash, and the bootloader's load address.
    pub const FLASH_BASE: u32 = 0x0800_0000;
    /// The v6 bootloader bank: pages 0-7.
    pub const BOOTLOADER_BYTES: u32 = 16 * 1024;
    /// What v4 and v5 occupied, and therefore how much of flash a board that has not been updated
    /// yet still keeps for its bootloader. Kept because a fielded fleet contains both.
    pub const BOOTLOADER_BYTES_LEGACY: u32 = 24 * 1024;
    /// Where a v6 board's application is linked.
    pub const APP_BASE: u32 = FLASH_BASE + BOOTLOADER_BYTES;
    /// Where a v4/v5 board's application is linked. A v6 bootloader will still start an image
    /// here when the new bank is blank, so this base stays current for as long as any board
    /// carries an old bootloader.
    pub const APP_BASE_LEGACY: u32 = FLASH_BASE + BOOTLOADER_BYTES_LEGACY;
    /// One past the end of flash on the 128 kB part.
    pub const FLASH_END: u32 = 0x0802_0000;
    /// First byte that is not application firmware, whichever base the application was linked
    /// for. The final three 2 KiB pages survive every firmware update.
    pub const PERSIST_BASE: u32 = 0x0801_E800;
    pub const IDENTITY_BASE: u32 = PERSIST_BASE;
    pub const SETTINGS_A_BASE: u32 = 0x0801_F000;
    pub const SETTINGS_B_BASE: u32 = 0x0801_F800;
    pub const FLASH_PAGE_BYTES: u32 = 2 * 1024;

    /// SRAM, 36 kB. `RAM_END` is one past the last byte, which is also where a valid initial
    /// stack pointer sits — the stack grows down, so `0x20009000` is correct rather than
    /// out of range.
    pub const RAM_BASE: u32 = 0x2000_0000;
    pub const RAM_END: u32 = 0x2000_9000;
    /// The 32 bytes at the top of SRAM that carry a board's bus address and serial across the
    /// reset from the application into the bootloader. Never initialised by startup code, because
    /// it has to survive the reset that carries it.
    pub const HANDOFF_ADDR: u32 = 0x2000_8FE0;
    pub const FIRMWARE_BYTES: u32 = PERSIST_BASE - FLASH_BASE;
    pub const PERSIST_BYTES: u32 = FLASH_END - PERSIST_BASE;

    /// Offset of the application descriptor from the application's base address: the first
    /// 16-byte-aligned address past the G070's 46-entry (0xB8-byte) vector table.
    pub const APP_DESCRIPTOR_OFFSET: usize = 0xC0;
    pub const APP_DESCRIPTOR_BYTES: usize = 0x38;
    pub const APP_DESCRIPTOR_MAGIC: &[u8; 8] = b"KC79APP1";
    pub const APP_VERSION_BYTES: usize = 0x28;

    /// How many bytes an application linked at `base` may occupy.
    ///
    /// Bounded by [`PERSIST_BASE`] rather than by the end of flash: the three durable pages above
    /// it hold the provisioning serial and the settings journals, and an image that reached them
    /// would be destroyed by the next settings write even if it fitted.
    pub fn app_bank_bytes(base: u32) -> u32 {
        PERSIST_BASE.saturating_sub(base)
    }

    /// Whether `base` is one of the two addresses an application is ever linked for.
    ///
    /// Anything else — most often `FLASH_BASE`, from a `no_bootloader` build — is refused rather
    /// than accommodated: such an image programs and verifies cleanly into an application slot and
    /// then never runs.
    pub fn is_app_base(base: u32) -> bool {
        base == APP_BASE || base == APP_BASE_LEGACY
    }

    /// Flash controller. `ACR +0x00`, `KEYR +0x08`, `OPTKEYR +0x0C`, `SR +0x10`, `CR +0x14`,
    /// `ECCR +0x18`, `OPTR +0x20`, `WRP1AR +0x2C`, `WRP1BR +0x30`.
    pub const FLASH_R_BASE: u32 = 0x4002_2000;
    pub const FLASH_KEYR: u32 = FLASH_R_BASE + 0x08;
    pub const FLASH_OPTKEYR: u32 = FLASH_R_BASE + 0x0C;
    pub const FLASH_SR: u32 = FLASH_R_BASE + 0x10;
    pub const FLASH_CR: u32 = FLASH_R_BASE + 0x14;
    pub const FLASH_OPTR: u32 = FLASH_R_BASE + 0x20;
    pub const FLASH_WRP1AR: u32 = FLASH_R_BASE + 0x2C;
    pub const FLASH_WRP1BR: u32 = FLASH_R_BASE + 0x30;

    /// Reset and clock control. `APBENR1` carries `DBGEN`, which is **0 out of reset** — so a
    /// poll that has not attached properly cannot read anything in the DBGMCU block.
    pub const RCC_BASE: u32 = 0x4002_1000;
    pub const RCC_APBENR1: u32 = RCC_BASE + 0x3C;
    /// Reset-cause flags. The most useful single diagnostic there is: it says whether the last
    /// reset was our NRST, the IWDG, or an option-byte reload.
    pub const RCC_CSR: u32 = RCC_BASE + 0x60;

    /// Debug MCU. `IDCODE +0x00`, `CR +0x04`, `APBFZ1 +0x08`, `APBFZ2 +0x0C`.
    pub const DBG_BASE: u32 = 0x4001_5800;
    pub const DBGMCU_IDCODE: u32 = DBG_BASE;
    pub const DBGMCU_CR: u32 = DBG_BASE + 0x04;
    pub const DBGMCU_APBFZ1: u32 = DBG_BASE + 0x08;

    /// 96-bit unique id. System memory, so it needs no peripheral clock.
    pub const UID_BASE: u32 = 0x1FFF_7590;
    /// Flash size in kB, 16-bit.
    pub const FLASHSIZE_BASE: u32 = 0x1FFF_75E0;

    /// ARMv6-M debug and system control. Architecture-fixed.
    pub const DHCSR: u32 = 0xE000_EDF0;
    pub const SCB_VTOR: u32 = 0xE000_ED08;
    pub const SCB_AIRCR: u32 = 0xE000_ED0C;
}

/// Register bit positions used by the flash and option-byte sequences.
pub mod bits {
    /// `FLASH_SR.BSY1` — an operation is in progress.
    pub const FLASH_SR_BSY1: u32 = 1 << 16;
    /// `FLASH_SR.CFGBSY` — a programming/erase configuration is still being taken.
    pub const FLASH_SR_CFGBSY: u32 = 1 << 18;
    /// Every `FLASH_SR` status bit that is `rc_w1`, so one write clears the lot.
    pub const FLASH_SR_CLEAR_MASK: u32 = 0x0000_C3FB;

    pub const FLASH_CR_OPTSTRT: u32 = 1 << 17;
    pub const FLASH_CR_OBL_LAUNCH: u32 = 1 << 27;
    pub const FLASH_CR_OPTLOCK: u32 = 1 << 30;
    pub const FLASH_CR_LOCK: u32 = 1 << 31;

    /// `DBGMCU_APBFZ1.DBG_IWDG_STOP` — freeze the independent watchdog while the core is halted.
    ///
    /// Belt and braces rather than the primary defence: attaching under reset halts at the reset
    /// vector, before the bootloader has run a single instruction, so in the normal path the
    /// IWDG never starts. This bit is what saves the pass when the reset did not take, or when
    /// `FLASH_OPTR.IWDG_SW` is 0 and the watchdog starts in hardware regardless.
    pub const DBG_IWDG_STOP: u32 = 1 << 12;

    /// `DHCSR.S_HALT`.
    pub const DHCSR_S_HALT: u32 = 1 << 17;
    /// `DHCSR.S_SLEEP`. PortalFW never sleeps, so this being set means something is wrong.
    pub const DHCSR_S_SLEEP: u32 = 1 << 18;
    /// `DHCSR.S_LOCKUP`.
    pub const DHCSR_S_LOCKUP: u32 = 1 << 19;
    /// `DHCSR.S_RESET_ST` — sticky, and cleared by reading `DHCSR`.
    ///
    /// Reading it at the start and end of the run-check window is what catches a board that is
    /// **resetting in a loop**: the exact "looks alive but is not working" failure that a
    /// run-check exists to find, and one that sampling a liveness counter alone would miss.
    pub const DHCSR_S_RESET_ST: u32 = 1 << 25;

    /// `FLASH_OPTR.RDP`, level 0.
    pub const OPTR_RDP_MASK: u32 = 0xFF;
    pub const OPTR_RDP_LEVEL0: u32 = 0xAA;
    pub const OPTR_RDP_LEVEL2: u32 = 0xCC;

    /// `FLASH_OPTR.IWDG_SW`. 0 means the watchdog is started by hardware at every reset,
    /// independent of firmware — which promotes [`DBG_IWDG_STOP`] from precaution to necessity.
    pub const OPTR_IWDG_SW: u32 = 1 << 16;
    /// `FLASH_OPTR.nBOOT_SEL`. 1 = boot selected by the option bits; 0 = boot selected by the
    /// **PA14-BOOT0 pin**, which the probe is driving as SWCLK when NRST releases. Must be 1.
    pub const OPTR_NBOOT_SEL: u32 = 1 << 24;
    pub const OPTR_NBOOT1: u32 = 1 << 25;
    pub const OPTR_NBOOT0: u32 = 1 << 26;
    /// `FLASH_OPTR.NRST_MODE[28:27]`. A value that puts PF2-NRST into GPIO silently degrades
    /// connect-under-reset into flashing a *running* target with a live watchdog.
    pub const OPTR_NRST_MODE_SHIFT: u32 = 27;
    pub const OPTR_NRST_MODE_MASK: u32 = 0b11 << OPTR_NRST_MODE_SHIFT;

    /// STM32G07x/G08x. Distinct from 0x466 (G03x/G04x), 0x456 (G05x/G061), 0x467 (G0B/G0C).
    pub const DEV_ID_STM32G07X: u32 = 0x460;
}

/// Flash and option-byte unlock keys.
pub mod keys {
    pub const KEY1: u32 = 0x4567_0123;
    pub const KEY2: u32 = 0xCDEF_89AB;
    pub const OPTKEY1: u32 = 0x0819_2A3B;
    pub const OPTKEY2: u32 = 0x4C5D_6E7F;
}

#[cfg(test)]
mod addr_tests {
    use super::addr::*;

    /// The firmware's own header, read at compile time. If the path breaks this file stops
    /// compiling, which is the intended failure mode — a drift test that silently skips itself is
    /// worse than no drift test.
    const HEADER: &str = include_str!("../../../../PortalBootloader/include/portal_flash_layout.h");

    /// Pull `#define NAME 0x...` out of the header.
    ///
    /// Deliberately only understands bare literals, which is the rule the header sets for itself
    /// precisely because four of its readers are not C compilers. A `#define` that grew an
    /// expression fails to parse here rather than being quietly mis-evaluated.
    fn define(name: &str) -> u64 {
        let mut found = None;
        for line in HEADER.lines() {
            let mut words = line.split_whitespace();
            if words.next() != Some("#define") || words.next() != Some(name) {
                continue;
            }
            let value = words
                .next()
                .unwrap_or_else(|| panic!("`#define {name}` has no value"));
            let parsed = value
                .strip_prefix("0x")
                .and_then(|hex| u64::from_str_radix(hex, 16).ok())
                .or_else(|| value.parse::<u64>().ok())
                .unwrap_or_else(|| {
                    panic!("`#define {name} {value}` is not a bare literal; see the header's note")
                });
            assert!(found.is_none(), "`{name}` is defined more than once");
            found = Some(parsed);
        }
        found.unwrap_or_else(|| panic!("`{name}` is not defined in portal_flash_layout.h"))
    }

    /// Every address this crate flashes to, against the header the firmware is built from.
    ///
    /// The numbers used to be duplicated as magic values across `platformio.ini`, `set_bank2.py`
    /// and the bootloader's `constants.h`, and nothing compared them. They are load-bearing in the
    /// way that a wrong one destroys a board's provisioning identity rather than failing a build,
    /// so this reads the definition rather than restating it.
    #[test]
    fn the_memory_map_agrees_with_the_firmware_header() {
        assert_eq!(u64::from(FLASH_BASE), define("PORTAL_FLASH_BASE"));
        assert_eq!(u64::from(FLASH_END), define("PORTAL_FLASH_END"));
        assert_eq!(
            u64::from(FLASH_PAGE_BYTES),
            define("PORTAL_FLASH_PAGE_BYTES")
        );
        assert_eq!(
            u64::from(BOOTLOADER_BYTES),
            define("PORTAL_BOOTLOADER_BYTES")
        );
        assert_eq!(
            u64::from(BOOTLOADER_BYTES_LEGACY),
            define("PORTAL_BOOTLOADER_BYTES_LEGACY")
        );
        assert_eq!(u64::from(APP_BASE), define("PORTAL_APP_BASE"));
        assert_eq!(u64::from(APP_BASE_LEGACY), define("PORTAL_APP_BASE_LEGACY"));
        // One name here, two in the header: the first byte that is not application firmware is
        // also the first durable page.
        assert_eq!(u64::from(PERSIST_BASE), define("PORTAL_APP_END"));
        assert_eq!(u64::from(IDENTITY_BASE), define("PORTAL_PERSIST_IDENTITY"));
        assert_eq!(
            u64::from(SETTINGS_A_BASE),
            define("PORTAL_PERSIST_SETTINGS_A")
        );
        assert_eq!(
            u64::from(SETTINGS_B_BASE),
            define("PORTAL_PERSIST_SETTINGS_B")
        );
        assert_eq!(u64::from(RAM_BASE), define("PORTAL_RAM_BASE"));
        assert_eq!(u64::from(RAM_END), define("PORTAL_RAM_END"));
        assert_eq!(u64::from(HANDOFF_ADDR), define("PORTAL_HANDOFF_ADDR"));
        assert_eq!(
            APP_DESCRIPTOR_OFFSET as u64,
            define("PORTAL_APP_DESCRIPTOR_OFFSET")
        );
        assert_eq!(
            APP_DESCRIPTOR_BYTES as u64,
            define("PORTAL_APP_DESCRIPTOR_BYTES")
        );
        assert_eq!(APP_VERSION_BYTES as u64, define("PORTAL_APP_VERSION_BYTES"));
        assert_eq!(u64::from(UID_BASE), define("PORTAL_UID_BASE"));

        // A string, so it is checked as one rather than parsed as a number.
        let quoted = format!("\"{}\"", std::str::from_utf8(APP_DESCRIPTOR_MAGIC).unwrap());
        assert!(
            HEADER
                .lines()
                .any(|line| line.contains("PORTAL_APP_DESCRIPTOR_MAGIC") && line.contains(&quoted)),
            "PORTAL_APP_DESCRIPTOR_MAGIC is not {quoted} in the header"
        );
    }

    #[test]
    fn the_memory_map_is_internally_consistent() {
        // Every boundary is an erase boundary, so every one of them lands on a page.
        for boundary in [
            FLASH_BASE,
            APP_BASE,
            APP_BASE_LEGACY,
            PERSIST_BASE,
            SETTINGS_A_BASE,
            SETTINGS_B_BASE,
            FLASH_END,
        ] {
            assert_eq!(
                boundary % FLASH_PAGE_BYTES,
                0,
                "{boundary:#010X} is not page-aligned"
            );
        }

        // Each bootloader bank ends exactly where its own application begins. Nothing derives one
        // from the other on the firmware side, because during the transition the v6 bootloader and
        // a legacy-base application are deliberately not adjacent.
        assert_eq!(FLASH_BASE + BOOTLOADER_BYTES, APP_BASE);
        assert_eq!(FLASH_BASE + BOOTLOADER_BYTES_LEGACY, APP_BASE_LEGACY);

        // The move buys 8 kB of application.
        assert_eq!(app_bank_bytes(APP_BASE), 108_544);
        assert_eq!(app_bank_bytes(APP_BASE_LEGACY), 100_352);
        assert_eq!(
            app_bank_bytes(APP_BASE) - app_bank_bytes(APP_BASE_LEGACY),
            BOOTLOADER_BYTES_LEGACY - BOOTLOADER_BYTES
        );
        // Both banks are a whole number of pages, since they are erased page by page.
        assert_eq!(app_bank_bytes(APP_BASE) % FLASH_PAGE_BYTES, 0);
        assert_eq!(app_bank_bytes(APP_BASE_LEGACY) % FLASH_PAGE_BYTES, 0);

        assert!(is_app_base(APP_BASE));
        assert!(is_app_base(APP_BASE_LEGACY));
        assert!(!is_app_base(FLASH_BASE));
        assert!(!is_app_base(APP_BASE + FLASH_PAGE_BYTES));

        assert_eq!(FIRMWARE_BYTES, 122 * 1024);
        assert_eq!(PERSIST_BYTES, 6 * 1024);
        assert_eq!(IDENTITY_BASE + FLASH_PAGE_BYTES, SETTINGS_A_BASE);
        assert_eq!(SETTINGS_A_BASE + FLASH_PAGE_BYTES, SETTINGS_B_BASE);
        assert_eq!(SETTINGS_B_BASE + FLASH_PAGE_BYTES, FLASH_END);
        assert_eq!(FLASH_END - FLASH_BASE, 128 * 1024);

        // The descriptor sits past the vector table rather than inside it, and the handoff block
        // occupies the very top of SRAM.
        assert!(APP_DESCRIPTOR_OFFSET >= 46 * 4);
        assert_eq!(HANDOFF_ADDR + 0x20, RAM_END);
        assert_eq!(RAM_END - RAM_BASE, 36 * 1024);
    }

    #[test]
    fn peripheral_bases_match_cmsis() {
        assert_eq!(FLASH_R_BASE, 0x4002_2000);
        assert_eq!(RCC_BASE, 0x4002_1000);
        assert_eq!(RCC_CSR, 0x4002_1060);
        assert_eq!(DBG_BASE, 0x4001_5800);
        assert_eq!(DBGMCU_APBFZ1, 0x4001_5808);
        assert_eq!(UID_BASE, 0x1FFF_7590);
        assert_eq!(FLASHSIZE_BASE, 0x1FFF_75E0);
    }
}
