//! What a bundle refuses, and why each refusal is worth its code.
//!
//! Every fault here is a way to flash a board that comes out looking fine and does not work.
//! They are cheap to check when a bundle is loaded at the bench and expensive to discover on a
//! unit that has already left it.

use portal_swd::addr;
use portal_swd::bits;
use portal_swd::image::{
    BundleFault, ImageBundle, OptionByteFault, OptionBytePolicy, Provenance, Region, RegionName,
    RunCheckSpec,
};

/// A minimal application image: vector table with a plausible reset vector, then padding.
fn application_bytes(len: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; len.max(8)];
    // [0] initial stack pointer, top of RAM.
    bytes[0..4].copy_from_slice(&0x2000_9000u32.to_le_bytes());
    // [1] reset vector: somewhere in the application bank, Thumb bit set.
    bytes[4..8].copy_from_slice(&(addr::APP_BASE + 0x241).to_le_bytes());
    bytes
}

fn good_bundle() -> ImageBundle {
    ImageBundle {
        bootloader: Region::new(RegionName::Bootloader, addr::FLASH_BASE, vec![0xAA; 22_708]),
        application: Region::new(
            RegionName::Application,
            addr::APP_BASE,
            application_bytes(60_000),
        ),
        option_bytes: OptionBytePolicy::default(),
        run_check: RunCheckSpec {
            liveness_address: 0x2000_0010,
            liveness_symbol: "g_liveness_counter".into(),
            ..RunCheckSpec::default()
        },
        provenance: Provenance::Synthetic,
    }
}

#[test]
fn a_well_formed_bundle_has_no_faults() {
    assert_eq!(good_bundle().validate(), vec![]);
}

// ---------------------------------------------------------------- the memory map

#[test]
fn an_application_linked_for_the_bootloader_slot_is_refused() {
    // This is the mistake that costs a bench session: `pio run -e no_bootloader` produces a
    // perfectly good binary that links at 0x08000000. Flashed into the application slot it
    // programs cleanly, verifies cleanly, and never runs.
    let mut bundle = good_bundle();
    bundle.application.bytes[4..8].copy_from_slice(&(addr::FLASH_BASE + 0x241).to_le_bytes());

    assert!(
        bundle
            .validate()
            .iter()
            .any(|f| matches!(f, BundleFault::BadResetVector { .. })),
        "an image linked for the bootloader slot was accepted"
    );
}

#[test]
fn a_reset_vector_without_the_thumb_bit_is_refused() {
    let mut bundle = good_bundle();
    bundle.application.bytes[4..8].copy_from_slice(&(addr::APP_BASE + 0x240).to_le_bytes());
    assert!(
        bundle
            .validate()
            .iter()
            .any(|f| matches!(f, BundleFault::BadResetVector { .. }))
    );
}

#[test]
fn a_bootloader_that_would_overlap_the_application_is_refused() {
    // Not hypothetical: the CubeIDE linker script says FLASH LENGTH = 28K for a 24 kB bank, so
    // an image between 24,576 and 28,672 bytes links cleanly today and silently overwrites the
    // start of the application.
    let mut bundle = good_bundle();
    bundle.bootloader.bytes = vec![0xAA; 26_000];

    assert!(
        bundle
            .validate()
            .contains(&BundleFault::BootloaderTooLarge {
                bytes: 26_000,
                limit: 24 * 1024,
            }),
        "a 26,000-byte bootloader was accepted into a 24 kB bank"
    );
}

#[test]
fn the_bank_boundary_is_exact() {
    let mut bundle = good_bundle();

    bundle.bootloader.bytes = vec![0xAA; 24 * 1024];
    assert_eq!(bundle.validate(), vec![], "exactly 24 kB should fit");

    bundle.bootloader.bytes.push(0xAA);
    assert!(!bundle.validate().is_empty(), "one byte over should not");
}

#[test]
fn an_application_larger_than_its_bank_is_refused() {
    let mut bundle = good_bundle();
    bundle.application.bytes = application_bytes(addr::APP_BANK_BYTES as usize + 1);
    assert!(
        bundle
            .validate()
            .iter()
            .any(|f| matches!(f, BundleFault::ApplicationTooLarge { .. }))
    );
}

#[test]
fn regions_must_be_at_the_addresses_the_firmware_build_links_them_at() {
    let mut bundle = good_bundle();
    bundle.application.load_address = 0x0800_4000;

    assert!(bundle.validate().contains(&BundleFault::WrongLoadAddress {
        region: RegionName::Application,
        expected: addr::APP_BASE,
        found: 0x0800_4000,
    }));
}

// ---------------------------------------------------------------- option bytes

#[test]
fn boot_from_the_swclk_pin_is_refused() {
    // nBOOT_SEL == 0 takes BOOT0 from PA14, which is the pin the probe drives as SWCLK. The
    // board can then leave connect-under-reset in the system ROM bootloader instead of ours,
    // intermittently, depending on what the probe happened to be doing.
    let mut bundle = good_bundle();
    bundle.option_bytes.optr &= !bits::OPTR_NBOOT_SEL;

    assert!(
        bundle
            .validate()
            .contains(&BundleFault::OptionBytes(OptionByteFault::BootFromPin))
    );
}

#[test]
fn disabling_the_reset_pin_is_refused() {
    // NRST in GPIO mode turns connect-under-reset into "attach to a running target with a live
    // watchdog, then erase it".
    let mut bundle = good_bundle();
    bundle.option_bytes.optr = (bundle.option_bytes.optr & !bits::OPTR_NRST_MODE_MASK)
        | (0b10 << bits::OPTR_NRST_MODE_SHIFT);

    assert!(
        bundle
            .validate()
            .contains(&BundleFault::OptionBytes(OptionByteFault::ResetPinDisabled))
    );
}

#[test]
fn readout_protection_can_never_ride_along_with_a_routine_write() {
    let mut bundle = good_bundle();
    bundle.option_bytes.optr_mask |= bits::OPTR_RDP_MASK;

    assert!(
        bundle
            .validate()
            .contains(&BundleFault::OptionBytes(OptionByteFault::RdpInMask)),
        "RDP is a mass-erase-scale change and must be its own, confirmed action"
    );
}

#[test]
fn rdp_is_preserved_even_if_the_golden_value_disagrees() {
    // A golden board read under RDP level 1 would carry a non-0xAA RDP field. Writing that back
    // to a virgin part would protect it. The merge keeps whatever the target already has.
    let policy = OptionBytePolicy {
        optr: 0xFFFF_FE00, // RDP field 0x00 -- level 1
        ..OptionBytePolicy::default()
    };
    let current = 0xFFFF_FEAA; // the target is at level 0

    assert_eq!(
        policy.desired(current) & bits::OPTR_RDP_MASK,
        bits::OPTR_RDP_LEVEL0,
        "the merge must never lower a target's readout protection"
    );
}

#[test]
fn only_masked_bits_are_touched() {
    let policy = OptionBytePolicy::default();
    // A target with some reserved/unmasked bits set differently from the golden value.
    let current = 0x1234_56AA;
    let desired = policy.desired(current);

    assert_eq!(
        desired & !policy.optr_mask & !bits::OPTR_RDP_MASK,
        current & !policy.optr_mask & !bits::OPTR_RDP_MASK,
        "bits outside the mask must be preserved exactly, including reserved ones"
    );
}

#[test]
fn a_matching_target_is_not_reprogrammed() {
    let policy = OptionBytePolicy::default();
    let already_right = policy.desired(0xFFFF_FEAA);
    assert!(
        !policy.needs_programming(already_right),
        "option flash has finite endurance; there is no reason to rewrite what is already there"
    );
}

#[test]
fn a_target_with_the_watchdog_in_hardware_mode_is_detected_as_differing() {
    // IWDG_SW == 0 means the watchdog starts at every reset regardless of firmware, which turns
    // the DBGMCU freeze from a precaution into a requirement. The tool should notice and correct
    // it rather than silently coping.
    let policy = OptionBytePolicy::default();
    let hardware_watchdog = policy.optr & !bits::OPTR_IWDG_SW;
    assert!(policy.needs_programming(hardware_watchdog));
    assert_ne!(policy.desired(hardware_watchdog) & bits::OPTR_IWDG_SW, 0);
}

// ---------------------------------------------------------------- the run check

#[test]
fn a_bundle_without_a_liveness_address_is_a_warning_not_a_refusal() {
    // It can be programmed; it just cannot be automatically run-checked. Refusing to flash a
    // perfectly good image because nothing has resolved a symbol out of its ELF yet would be the
    // tail wagging the dog -- and every image discovered from the build tree is in that state
    // until an ELF reader exists.
    let mut bundle = good_bundle();
    bundle.run_check.liveness_address = 0;

    assert_eq!(
        bundle.validate(),
        vec![],
        "an absent liveness address must not block a flash"
    );
    assert!(
        bundle.warnings().contains(&BundleFault::NoLivenessAddress),
        "but it must still be said out loud: without a counter to watch, a run-check could only \
         prove the bootloader jumped, and a board spinning in HardFault_Handler would pass"
    );
}

#[test]
fn a_liveness_address_outside_ram_is_refused() {
    let mut bundle = good_bundle();
    bundle.run_check.liveness_address = addr::APP_BASE;
    assert!(
        bundle
            .validate()
            .iter()
            .any(|f| matches!(f, BundleFault::LivenessNotInRam { .. }))
    );
}

#[test]
fn the_run_check_expects_the_application_vector_table() {
    assert_eq!(
        RunCheckSpec::default().vtor,
        addr::APP_BASE,
        "VTOR is what distinguishes 'the bootloader handed over to us' from 'it handed over to \
         the system ROM'"
    );
}

// ---------------------------------------------------------------- hashing and readback

#[test]
fn the_bundle_hash_covers_the_bytes_and_the_metadata() {
    let base = good_bundle();
    let base_hash = base.sha256();

    let mut different_bytes = good_bundle();
    different_bytes.application.bytes[100] ^= 0xFF;
    assert_ne!(base_hash, different_bytes.sha256());

    let mut different_liveness = good_bundle();
    different_liveness.run_check.liveness_address = 0x2000_0020;
    assert_ne!(
        base_hash,
        different_liveness.sha256(),
        "the run-check spec must be bound to the hash, or an address could drift away from the \
         image it was resolved against"
    );

    assert_eq!(base_hash, good_bundle().sha256(), "hashing must be stable");
}

#[test]
fn the_expected_flash_image_is_the_whole_device() {
    let bundle = good_bundle();
    let image = bundle.expected_flash_image();

    assert_eq!(image.len(), 128 * 1024);
    assert_eq!(&image[..8], &bundle.bootloader.bytes[..8]);

    // The gap between the end of the bootloader and the application bank is erased flash, not
    // stale bytes -- the pass mass-erases, so a readback must expect 0xFF there.
    let gap = bundle.bootloader.bytes.len()..(addr::BOOTLOADER_BYTES as usize);
    assert!(image[gap].iter().all(|&b| b == 0xFF));

    let app_start = addr::BOOTLOADER_BYTES as usize;
    assert_eq!(
        &image[app_start..app_start + 8],
        &bundle.application.bytes[..8]
    );

    let tail = app_start + bundle.application.bytes.len();
    assert!(image[tail..].iter().all(|&b| b == 0xFF));
}

#[test]
fn the_manifest_round_trips_through_json() {
    let bundle = good_bundle();
    let manifest = bundle.manifest();
    let json = serde_json::to_string_pretty(&manifest).unwrap();
    let back: portal_swd::image::Manifest = serde_json::from_str(&json).unwrap();
    assert_eq!(manifest, back);

    assert_eq!(manifest.regions.len(), 2);
    assert_eq!(manifest.regions[0].load_address, addr::FLASH_BASE);
    assert_eq!(manifest.regions[1].load_address, addr::APP_BASE);
}
