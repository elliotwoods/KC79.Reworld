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
//! | Bootloader | `0x08000000`, 24 kB — RS485 field updater, and what starts the IWDG |
//! | Application | `0x08006000`, 104 kB — built by `PortalFW`, sets `VTOR` to its own base |
//! | Debug | SWD on PA13/PA14. **PA14 is also BOOT0**, which is why `nBOOT_SEL` matters |

pub mod artefacts;
pub mod device;
pub mod image;
pub mod machine;
pub mod program;
pub mod rig;
pub mod symbols;

#[cfg(feature = "probe")]
pub mod probe;

pub use artefacts::{Artefact, Discovery, Origin, discover};
pub use device::{DeviceImage, DeviceReport, Layout, OptionBytes, OptionWarning, VectorTable};

pub use image::{BundleFault, ImageBundle, OptionBytePolicy, Region, RegionName, RunCheckSpec};
pub use machine::{Action, Cue, Input, Machine, Millis, Pass, Phase, Timing};
#[cfg(feature = "probe")]
pub use probe::{ProbeDescriptor, ProbeRsRig, list_probes};
pub use rig::{
    FlashReport, Presence, ProbeInfo, Rig, RigError, RigErrorKind, RunCheckFault, RunCheckReport,
    SimRig, Step, Trigger,
};

/// Fixed addresses on the STM32G070, verified against the CMSIS device header
/// (`framework-arduinoststm32/system/Drivers/CMSIS/Device/ST/STM32G0xx/Include/stm32g070xx.h`)
/// rather than recalled.
///
/// The `FLASH_OPTR` *field* positions are the exception: those come from libopencm3, not from
/// ST, and must be checked against RM0454 §3.4.1 before anything writes an option byte. Getting
/// `nBOOT_SEL` wrong changes how the part boots.
pub mod addr {
    /// Start of flash, and the bootloader's load address.
    pub const FLASH_BASE: u32 = 0x0800_0000;
    /// The bootloader bank is the first 24 kB (`BOOTLOADER_SIZE` in the bootloader's
    /// `constants.h`).
    pub const BOOTLOADER_BYTES: u32 = 24 * 1024;
    /// Where the application is linked (`board_upload.offset_address`, and `set_bank2.py`'s
    /// `VECT_TAB_OFFSET`/`LD_FLASH_OFFSET`).
    pub const APP_BASE: u32 = FLASH_BASE + BOOTLOADER_BYTES;
    /// One past the end of flash on the 128 kB part.
    pub const FLASH_END: u32 = 0x0802_0000;

    /// SRAM, 36 kB. `RAM_END` is one past the last byte, which is also where a valid initial
    /// stack pointer sits — the stack grows down, so `0x20009000` is correct rather than
    /// out of range.
    pub const RAM_BASE: u32 = 0x2000_0000;
    pub const RAM_END: u32 = 0x2000_9000;
    /// Bytes available to the application.
    pub const APP_BANK_BYTES: u32 = FLASH_END - APP_BASE;

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

    #[test]
    fn the_memory_map_agrees_with_the_firmware_build() {
        // These four numbers are duplicated as magic values across PortalFW/platformio.ini,
        // set_bank2.py and the bootloader's constants.h. If any of them moves, this fails here
        // rather than as a corrupt device.
        assert_eq!(APP_BASE, 0x0800_6000);
        assert_eq!(BOOTLOADER_BYTES, 0x6000);
        assert_eq!(APP_BANK_BYTES, 106_496);
        assert_eq!(FLASH_END - FLASH_BASE, 128 * 1024);
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
