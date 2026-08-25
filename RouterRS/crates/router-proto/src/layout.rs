//! The Portal flash and RAM map, mirrored from the firmware's own header.
//!
//! `PortalBootloader/include/portal_flash_layout.h` is the definition; this module is a copy, and
//! the test at the bottom is what makes "copy" mean something. It reads the header as text and
//! asserts every constant here against it, so a change on the firmware side that nobody mirrored
//! fails `cargo test` rather than corrupting a board.
//!
//! # Two application bases
//!
//! A fleet contains boards running bootloader v4/v5, whose application starts at
//! [`APP_BASE_LEGACY`] (`0x08006000`), and boards running v6, whose application starts at
//! [`APP_BASE`] (`0x08004000`) because the bootloader shrank from 24 kB to 16 kB. Both are
//! current; neither is deprecated while any board is still on the old bootloader. Host code must
//! therefore treat "where is the application" as a property of *the target*, discovered from its
//! bootloader, and "where was this image linked" as a property of *the image*, read from its
//! descriptor -- never as a constant. Everything that takes a base address in this crate does so
//! for that reason.

/// Start of flash, and the bootloader's load address.
pub const FLASH_BASE: u32 = 0x0800_0000;
/// One past the end of flash on the 128 kB part.
pub const FLASH_END: u32 = 0x0802_0000;
/// Erase granularity. The whole map is expressed in these.
pub const FLASH_PAGE_BYTES: u32 = 0x800;

/// The v6 bootloader bank: pages 0-7.
pub const BOOTLOADER_BYTES: u32 = 0x4000;
/// The v4/v5 bootloader bank: pages 0-11.
pub const BOOTLOADER_BYTES_LEGACY: u32 = 0x6000;

/// Where a v6 board's application is linked.
pub const APP_BASE: u32 = 0x0800_4000;
/// Where a v4/v5 board's application is linked. A v6 bootloader will still start an image here
/// when the new base is blank, which is what makes an in-band bootloader replacement survivable.
pub const APP_BASE_LEGACY: u32 = 0x0800_6000;
/// First byte that is not application firmware: the first of the three durable pages.
pub const APP_END: u32 = 0x0801_E800;

/// The append-only provisioning identity journal (serial number).
pub const PERSIST_IDENTITY: u32 = 0x0801_E800;
/// The A/B settings journals.
pub const PERSIST_SETTINGS_A: u32 = 0x0801_F000;
pub const PERSIST_SETTINGS_B: u32 = 0x0801_F800;

pub const RAM_BASE: u32 = 0x2000_0000;
pub const RAM_END: u32 = 0x2000_9000;

/// The 32-byte block in the top of SRAM that carries a board's identity across the reset from the
/// application into the bootloader.
pub const HANDOFF_ADDR: u32 = 0x2000_8FE0;
pub const HANDOFF_BYTES: u32 = 0x20;
/// `"K79H"` little-endian.
pub const HANDOFF_MAGIC: u32 = 0x4839_374B;
pub const HANDOFF_VERSION: u8 = 1;

pub const HANDOFF_REQUEST_NONE: u8 = 0;
pub const HANDOFF_REQUEST_STAY: u8 = 1;
pub const HANDOFF_REQUEST_RUN_NOW: u8 = 2;
pub const HANDOFF_FLAG_SERIAL_VALID: u8 = 1;

/// Offset of the application descriptor from the application's base address.
pub const APP_DESCRIPTOR_OFFSET: usize = 0xC0;
pub const APP_DESCRIPTOR_BYTES: usize = 0x38;
pub const APP_DESCRIPTOR_MAGIC: &[u8; 8] = b"KC79APP1";
pub const APP_VERSION_BYTES: usize = 0x28;

/// Version reported by the bootloader's `status` verb.
pub const BL_PROTO_VERSION: u8 = 6;
/// Largest firmware-frame payload a v6 bootloader accepts.
pub const BL_CHUNK_MAX: usize = 0x100;
/// Flash programs a double-word at a time, so every image length and frame offset is a multiple
/// of this.
pub const FLASH_GRANULE: usize = 8;

/// 96-bit unique id in system memory.
pub const UID_BASE: u32 = 0x1FFF_7590;

/// How many application bytes are available to an image linked at `base`.
///
/// Always bounded by [`APP_END`] rather than by the end of flash: the three durable pages above it
/// hold the provisioning serial and settings, and an image that reached them would be destroyed by
/// the next settings write even if it fitted.
pub fn app_bank_bytes(base: u32) -> usize {
    APP_END.saturating_sub(base) as usize
}

/// Whether `base` is one of the two addresses an application is ever linked for.
pub fn is_app_base(base: u32) -> bool {
    base == APP_BASE || base == APP_BASE_LEGACY
}

/// The bootloader bank size that pairs with an application at `base`.
pub fn bootloader_bytes_for(base: u32) -> Option<u32> {
    match base {
        APP_BASE => Some(BOOTLOADER_BYTES),
        APP_BASE_LEGACY => Some(BOOTLOADER_BYTES_LEGACY),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The firmware header, read at compile time. If the path breaks, this file stops compiling,
    /// which is the intended failure mode -- a silently skipped drift test is worse than none.
    const HEADER: &str = include_str!("../../../../PortalBootloader/include/portal_flash_layout.h");

    /// Pull `#define NAME 0x...` out of the header.
    ///
    /// Deliberately only understands bare literals, matching the rule the header documents for
    /// itself. A `#define` that grew an expression would fail to parse here rather than being
    /// quietly mis-evaluated by a parser that is not a C compiler.
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
                    panic!("`#define {name} {value}` is not a bare literal; see the header's own note")
                });
            assert!(found.is_none(), "`{name}` is defined more than once");
            found = Some(parsed);
        }
        found.unwrap_or_else(|| panic!("`{name}` is not defined in portal_flash_layout.h"))
    }

    #[test]
    fn every_constant_matches_the_firmware_header() {
        assert_eq!(u64::from(FLASH_BASE), define("PORTAL_FLASH_BASE"));
        assert_eq!(u64::from(FLASH_END), define("PORTAL_FLASH_END"));
        assert_eq!(u64::from(FLASH_PAGE_BYTES), define("PORTAL_FLASH_PAGE_BYTES"));
        assert_eq!(u64::from(BOOTLOADER_BYTES), define("PORTAL_BOOTLOADER_BYTES"));
        assert_eq!(
            u64::from(BOOTLOADER_BYTES_LEGACY),
            define("PORTAL_BOOTLOADER_BYTES_LEGACY")
        );
        assert_eq!(u64::from(APP_BASE), define("PORTAL_APP_BASE"));
        assert_eq!(u64::from(APP_BASE_LEGACY), define("PORTAL_APP_BASE_LEGACY"));
        assert_eq!(u64::from(APP_END), define("PORTAL_APP_END"));
        assert_eq!(u64::from(PERSIST_IDENTITY), define("PORTAL_PERSIST_IDENTITY"));
        assert_eq!(u64::from(PERSIST_SETTINGS_A), define("PORTAL_PERSIST_SETTINGS_A"));
        assert_eq!(u64::from(PERSIST_SETTINGS_B), define("PORTAL_PERSIST_SETTINGS_B"));
        assert_eq!(u64::from(RAM_BASE), define("PORTAL_RAM_BASE"));
        assert_eq!(u64::from(RAM_END), define("PORTAL_RAM_END"));
        assert_eq!(u64::from(HANDOFF_ADDR), define("PORTAL_HANDOFF_ADDR"));
        assert_eq!(u64::from(HANDOFF_BYTES), define("PORTAL_HANDOFF_BYTES"));
        assert_eq!(u64::from(HANDOFF_MAGIC), define("PORTAL_HANDOFF_MAGIC"));
        assert_eq!(u64::from(HANDOFF_VERSION), define("PORTAL_HANDOFF_VERSION"));
        assert_eq!(APP_DESCRIPTOR_OFFSET as u64, define("PORTAL_APP_DESCRIPTOR_OFFSET"));
        assert_eq!(APP_DESCRIPTOR_BYTES as u64, define("PORTAL_APP_DESCRIPTOR_BYTES"));
        assert_eq!(APP_VERSION_BYTES as u64, define("PORTAL_APP_VERSION_BYTES"));
        assert_eq!(u64::from(BL_PROTO_VERSION), define("PORTAL_BL_PROTO_VERSION"));
        assert_eq!(BL_CHUNK_MAX as u64, define("PORTAL_BL_CHUNK_MAX"));
        assert_eq!(FLASH_GRANULE as u64, define("PORTAL_FLASH_GRANULE"));
        assert_eq!(u64::from(UID_BASE), define("PORTAL_UID_BASE"));
    }

    #[test]
    fn the_descriptor_magic_matches_the_header() {
        let quoted = format!("\"{}\"", std::str::from_utf8(APP_DESCRIPTOR_MAGIC).unwrap());
        assert!(
            HEADER
                .lines()
                .any(|line| line.contains("PORTAL_APP_DESCRIPTOR_MAGIC") && line.contains(&quoted)),
            "PORTAL_APP_DESCRIPTOR_MAGIC is not {quoted} in the header"
        );
    }

    #[test]
    fn the_map_is_internally_consistent() {
        // Every boundary lands on a page, because every one of them is an erase boundary.
        for boundary in [
            FLASH_BASE,
            BOOTLOADER_BYTES,
            BOOTLOADER_BYTES_LEGACY,
            APP_BASE,
            APP_BASE_LEGACY,
            APP_END,
            PERSIST_IDENTITY,
            PERSIST_SETTINGS_A,
            PERSIST_SETTINGS_B,
            FLASH_END,
        ] {
            assert_eq!(boundary % FLASH_PAGE_BYTES, 0, "0x{boundary:08X} is not page-aligned");
        }

        // Each bootloader bank ends exactly where its application begins.
        assert_eq!(FLASH_BASE + BOOTLOADER_BYTES, APP_BASE);
        assert_eq!(FLASH_BASE + BOOTLOADER_BYTES_LEGACY, APP_BASE_LEGACY);

        // The three durable pages sit above the application and fill flash to the end.
        assert_eq!(PERSIST_IDENTITY, APP_END);
        assert_eq!(PERSIST_IDENTITY + FLASH_PAGE_BYTES, PERSIST_SETTINGS_A);
        assert_eq!(PERSIST_SETTINGS_A + FLASH_PAGE_BYTES, PERSIST_SETTINGS_B);
        assert_eq!(PERSIST_SETTINGS_B + FLASH_PAGE_BYTES, FLASH_END);
        assert_eq!(FLASH_END - FLASH_BASE, 128 * 1024);

        // The handoff block occupies the very top of SRAM, so excluding it from a linker script is
        // a matter of shortening RAM rather than carving a hole in it.
        assert_eq!(HANDOFF_ADDR + HANDOFF_BYTES, RAM_END);
        assert_eq!(RAM_END - RAM_BASE, 36 * 1024);

        // The descriptor sits past the G070's 46-entry vector table.
        assert!(APP_DESCRIPTOR_OFFSET >= 46 * 4);

        // The move is worth making: 8 kB more application.
        assert_eq!(app_bank_bytes(APP_BASE), 108_544);
        assert_eq!(app_bank_bytes(APP_BASE_LEGACY), 100_352);
        assert_eq!(
            app_bank_bytes(APP_BASE) - app_bank_bytes(APP_BASE_LEGACY),
            (BOOTLOADER_BYTES_LEGACY - BOOTLOADER_BYTES) as usize
        );

        // Both banks are a whole number of pages, since `begin` erases them page by page.
        assert_eq!(app_bank_bytes(APP_BASE) % FLASH_PAGE_BYTES as usize, 0);
        assert_eq!(app_bank_bytes(APP_BASE_LEGACY) % FLASH_PAGE_BYTES as usize, 0);
        assert_eq!(app_bank_bytes(APP_BASE) / FLASH_PAGE_BYTES as usize, 53);
    }

    #[test]
    fn base_helpers_reject_anything_else() {
        assert!(is_app_base(APP_BASE));
        assert!(is_app_base(APP_BASE_LEGACY));
        assert!(!is_app_base(FLASH_BASE));
        assert!(!is_app_base(APP_BASE + FLASH_PAGE_BYTES));
        assert_eq!(bootloader_bytes_for(APP_BASE), Some(BOOTLOADER_BYTES));
        assert_eq!(
            bootloader_bytes_for(APP_BASE_LEGACY),
            Some(BOOTLOADER_BYTES_LEGACY)
        );
        assert_eq!(bootloader_bytes_for(FLASH_BASE), None);
    }
}
