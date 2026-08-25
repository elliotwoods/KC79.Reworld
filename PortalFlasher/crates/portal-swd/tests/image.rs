//! What a bundle refuses, and why each refusal is worth its code.
//!
//! Every fault here is a way to flash a board that comes out looking fine and does not work.
//! They are cheap to check when a bundle is loaded at the bench and expensive to discover on a
//! unit that has already left it.

use portal_swd::addr;
use portal_swd::bits;
use portal_swd::image::{
    BaseSource, BundleFault, ImageBundle, OptionByteFault, OptionBytePolicy, Provenance, Region,
    RegionName, RunCheckSpec, image_base, read_descriptor,
};

/// A minimal application image: vector table, then the descriptor stating which bank it was linked
/// for, then padding — the shape `PortalFW/ldscript_app.ld` produces.
fn application_bytes(base: u32, len: usize) -> Vec<u8> {
    let mut bytes = descriptorless_bytes(base, len);
    let at = addr::APP_DESCRIPTOR_OFFSET;
    bytes[at..at + 8].copy_from_slice(addr::APP_DESCRIPTOR_MAGIC);
    bytes[at + 8..at + 12].copy_from_slice(&base.to_le_bytes());
    bytes[at + 12..at + 16].copy_from_slice(&0u32.to_le_bytes());
    let version = b"Portal v2026-08-25_19.19 ea08436+";
    bytes[at + 16..at + 16 + version.len()].copy_from_slice(version);
    bytes
}

/// The same without a descriptor: every application built before the descriptor existed, which is
/// what the fielded fleet is running.
///
/// The reset vector is `| 1`, not `+ 1`: the Thumb bit is a bit, and an entry offset that happened
/// to be odd would have it cleared by an addition rather than set.
fn descriptorless_bytes(base: u32, len: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; len.max(addr::APP_DESCRIPTOR_OFFSET + addr::APP_DESCRIPTOR_BYTES)];
    // [0] initial stack pointer, top of RAM.
    bytes[0..4].copy_from_slice(&0x2000_9000u32.to_le_bytes());
    // [1] reset vector: somewhere in the application bank, Thumb bit set.
    bytes[4..8].copy_from_slice(&((base + 0x240) | 1).to_le_bytes());
    bytes
}

/// A bootloader image that says which generation it is, the way a real one does — the banner is a
/// plain string literal, so it survives into the binary.
fn bootloader_bytes(version: u32, len: usize) -> Vec<u8> {
    let mut bytes = vec![0xAA; len];
    let banner = format!("Bootloader v{version}\0");
    bytes[0x200..0x200 + banner.len()].copy_from_slice(banner.as_bytes());
    bytes
}

/// A matched v6 pair: a 16 kB-class bootloader and an application linked for `0x08004000`.
fn good_bundle() -> ImageBundle {
    bundle(bootloader_bytes(6, 14_000), addr::APP_BASE)
}

/// A matched v4/v5 pair: a 24 kB-class bootloader and an application linked for `0x08006000`.
/// Every bit as current as the one above, because a fielded fleet contains both.
fn legacy_bundle() -> ImageBundle {
    bundle(bootloader_bytes(5, 22_708), addr::APP_BASE_LEGACY)
}

fn bundle(boot: Vec<u8>, base: u32) -> ImageBundle {
    ImageBundle {
        bootloader: Region::new(RegionName::Bootloader, addr::FLASH_BASE, boot),
        application: Region::new(
            RegionName::Application,
            base,
            application_bytes(base, 60_000),
        ),
        option_bytes: OptionBytePolicy::default(),
        run_check: RunCheckSpec {
            liveness_address: 0x2000_0010,
            liveness_symbol: "g_liveness_counter".into(),
            ..RunCheckSpec::for_base(base)
        },
        provenance: Provenance::Synthetic,
        unselected: portal_swd::image::Unselected::Erase,
    }
}

#[test]
fn a_well_formed_bundle_has_no_faults() {
    assert_eq!(good_bundle().validate(), vec![]);
    assert_eq!(good_bundle().warnings(), vec![]);
    assert_eq!(
        legacy_bundle().validate(),
        vec![],
        "a v4/v5 pair is not deprecated: it is what a board that has not been updated needs"
    );
}

// ---------------------------------------------------------------- what an image says about itself

#[test]
fn a_descriptor_states_the_base_it_was_linked_for() {
    let bytes = application_bytes(addr::APP_BASE, 60_000);
    let descriptor = read_descriptor(&bytes).expect("a v6-era image carries one");
    assert_eq!(descriptor.base, addr::APP_BASE);
    assert_eq!(descriptor.flags, 0);
    assert_eq!(descriptor.version, "Portal v2026-08-25_19.19 ea08436+");
    assert_eq!(
        image_base(&bytes),
        Some((addr::APP_BASE, BaseSource::Descriptor))
    );

    let legacy = application_bytes(addr::APP_BASE_LEGACY, 60_000);
    assert_eq!(
        image_base(&legacy),
        Some((addr::APP_BASE_LEGACY, BaseSource::Descriptor)),
        "the same descriptor names the other bank when that is what the image was built for"
    );
}

/// The case the descriptor exists for: the two banks overlap, so a new-base image's reset vector
/// can land inside the legacy bank and inference alone would read it as legacy.
#[test]
fn the_descriptor_beats_the_reset_vector() {
    let mut bytes = application_bytes(addr::APP_BASE, 60_000);
    bytes[4..8].copy_from_slice(&((addr::APP_BASE + 0x4000) | 1).to_le_bytes());
    assert!(
        (addr::APP_BASE_LEGACY..addr::PERSIST_BASE).contains(&(addr::APP_BASE + 0x4000)),
        "this test only means something if inference would get it wrong"
    );
    assert_eq!(
        image_base(&bytes),
        Some((addr::APP_BASE, BaseSource::Descriptor))
    );
}

#[test]
fn an_image_with_no_descriptor_can_only_ever_be_legacy() {
    // Every application built before the descriptor existed, and the reason inference has one
    // possible answer rather than two: the new base did not exist when those images were built,
    // so concluding it would be a guess -- and the wrong guess programs, verifies and hard-faults.
    let bytes = descriptorless_bytes(addr::APP_BASE_LEGACY, 60_000);
    assert_eq!(read_descriptor(&bytes), None);
    assert_eq!(
        image_base(&bytes),
        Some((addr::APP_BASE_LEGACY, BaseSource::InferredLegacy))
    );
}

#[test]
fn an_image_that_says_nothing_usable_is_refused_rather_than_guessed() {
    // A `no_bootloader` build: linked at 0x08000000, it programs and verifies cleanly into an
    // application slot and never runs. Its reset vector is in neither application bank, so there
    // is nothing to infer from and nothing is invented.
    assert_eq!(
        image_base(&descriptorless_bytes(addr::FLASH_BASE, 60_000)),
        None
    );

    // No Thumb bit: not a Cortex-M entry point, whatever else is true of the image.
    let mut blunt = descriptorless_bytes(addr::APP_BASE_LEGACY, 60_000);
    blunt[4] &= !1;
    assert_eq!(image_base(&blunt), None);

    // Too short to hold a vector table at all.
    assert_eq!(image_base(&[]), None);
    assert_eq!(image_base(&[0u8; 4]), None);

    // Right offset, wrong magic: a damaged descriptor must never be read as a valid one, so this
    // falls through to inference rather than reading whatever follows as a base address.
    let mut damaged = application_bytes(addr::APP_BASE_LEGACY, 60_000);
    damaged[addr::APP_DESCRIPTOR_OFFSET] = b'X';
    assert_eq!(read_descriptor(&damaged), None);
    assert_eq!(
        image_base(&damaged),
        Some((addr::APP_BASE_LEGACY, BaseSource::InferredLegacy))
    );

    // A descriptor naming a base no bootloader would start.
    let mut impossible = application_bytes(addr::APP_BASE, 60_000);
    let at = addr::APP_DESCRIPTOR_OFFSET + 8;
    impossible[at..at + 4].copy_from_slice(&0x0800_5000u32.to_le_bytes());
    assert_eq!(image_base(&impossible), None);

    // An image too short to reach the descriptor offset is simply one without a descriptor.
    let short = application_bytes(addr::APP_BASE_LEGACY, 60_000)[..0x40].to_vec();
    assert_eq!(read_descriptor(&short), None);
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

/// The reset vector is read against the base this bundle is loading the image at, not against the
/// lower of the two -- otherwise a legacy-base image entering below `0x08006000` would pass.
#[test]
fn a_reset_vector_below_the_bank_it_is_being_loaded_into_is_refused() {
    let mut bundle = legacy_bundle();
    bundle.application.bytes[4..8].copy_from_slice(&((addr::APP_BASE + 0x240) | 1).to_le_bytes());
    assert!(bundle.validate().contains(&BundleFault::BadResetVector {
        found: (addr::APP_BASE + 0x240) | 1,
        base: addr::APP_BASE_LEGACY,
    }));
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
fn a_bootloader_larger_than_any_bootloader_bank_is_refused() {
    // Not hypothetical: the CubeIDE linker script says FLASH LENGTH = 28K for a 24 kB bank, so
    // an image between 24,576 and 28,672 bytes links cleanly today and silently overwrites the
    // start of the application.
    let mut bundle = legacy_bundle();
    bundle.bootloader.bytes = bootloader_bytes(5, 26_000);

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

/// A v6 bootloader is held to 16 kB, and the number comes from the image's own banner. An older
/// one -- or one whose banner cannot be read -- is held to 24 kB, which is what keeps the committed
/// reference image passing.
#[test]
fn a_v6_bootloader_is_held_to_the_smaller_bank() {
    let mut bundle = good_bundle();
    bundle.bootloader.bytes = bootloader_bytes(6, 17_000);
    assert!(
        bundle
            .validate()
            .contains(&BundleFault::BootloaderTooLarge {
                bytes: 17_000,
                limit: 16 * 1024,
            })
    );

    // The same bytes with a v5 banner are a legal v5 bootloader -- paired with the application
    // base that goes with it.
    let mut older = legacy_bundle();
    older.bootloader.bytes = bootloader_bytes(5, 17_000);
    assert_eq!(older.validate(), vec![]);

    // And an image with no banner at all is held to the larger bank rather than the smaller: an
    // unidentifiable bootloader must not be assumed to be the newest one.
    let mut anonymous = legacy_bundle();
    anonymous.bootloader.bytes = vec![0xAA; 17_000];
    assert_eq!(anonymous.validate(), vec![]);
}

#[test]
fn the_bank_boundary_is_exact() {
    let mut v6 = good_bundle();
    v6.bootloader.bytes = bootloader_bytes(6, 16 * 1024);
    assert_eq!(v6.validate(), vec![], "exactly 16 kB should fit a v6 bank");
    v6.bootloader.bytes.push(0xAA);
    assert!(!v6.validate().is_empty(), "one byte over should not");

    let mut legacy = legacy_bundle();
    legacy.bootloader.bytes = bootloader_bytes(5, 24 * 1024);
    assert_eq!(
        legacy.validate(),
        vec![],
        "exactly 24 kB should fit a v5 bank"
    );
    legacy.bootloader.bytes.push(0xAA);
    assert!(!legacy.validate().is_empty(), "one byte over should not");
}

/// The pairing that bricks a board, and the reason a bundle is checked as a *pair* rather than as
/// two independently valid halves.
#[test]
fn a_legacy_bootloader_beside_a_new_base_application_is_refused() {
    let mut bundle = good_bundle();
    // Legal on its own -- 22,708 bytes is a perfectly good v4/v5 bootloader -- and fatal here:
    // eleven pages reach 0x08006000, over the top of an application based at 0x08004000.
    bundle.bootloader.bytes = bootloader_bytes(5, 22_708);

    assert!(
        bundle
            .validate()
            .contains(&BundleFault::BootloaderOverlapsApplication {
                bootloader_end: addr::APP_BASE_LEGACY,
                app_base: addr::APP_BASE,
            }),
        "the bootloader's tail would land on the application's vector table"
    );
    assert!(
        !bundle
            .validate()
            .contains(&BundleFault::BootloaderTooLarge {
                bytes: 22_708,
                limit: 24 * 1024,
            }),
        "the bootloader is not too large for itself; the pairing is what is wrong"
    );
}

/// The rule is about pages, not bytes: flash is erased a page at a time, so a bootloader one byte
/// into the application's first page takes the whole page with it.
#[test]
fn the_pair_rule_counts_whole_pages() {
    let mut bundle = good_bundle();

    bundle.bootloader.bytes = bootloader_bytes(6, 16 * 1024);
    assert_eq!(
        bundle.validate(),
        vec![],
        "eight pages stop exactly at 0x08004000"
    );

    bundle.bootloader.bytes = vec![0xAA; 16 * 1024 + 1];
    assert!(
        bundle
            .validate()
            .contains(&BundleFault::BootloaderOverlapsApplication {
                bootloader_end: addr::APP_BASE + addr::FLASH_PAGE_BYTES,
                app_base: addr::APP_BASE,
            }),
        "one byte over claims the application's first page"
    );
}

/// A bootloader-only pass says nothing about where the application on the board is, so the pair
/// rule has no pair to judge and must not invent one.
#[test]
fn the_pair_rule_needs_a_pair() {
    let mut boot_only = legacy_bundle();
    boot_only.application = Region::new(RegionName::Application, addr::APP_BASE, Vec::new());
    assert_eq!(
        boot_only.validate(),
        vec![],
        "installing a 24 kB bootloader on its own must stay possible"
    );
}

#[test]
fn an_application_larger_than_its_bank_is_refused() {
    let mut bundle = good_bundle();
    let limit = addr::app_bank_bytes(addr::APP_BASE) as usize;
    bundle.application.bytes = application_bytes(addr::APP_BASE, limit + 1);
    assert!(
        bundle
            .validate()
            .contains(&BundleFault::ApplicationTooLarge {
                bytes: limit + 1,
                limit,
            })
    );

    // The legacy bank is 8 kB smaller, so an image that fits the new one need not fit it.
    let mut legacy = legacy_bundle();
    legacy.application.bytes = application_bytes(addr::APP_BASE_LEGACY, limit);
    assert!(
        legacy
            .validate()
            .contains(&BundleFault::ApplicationTooLarge {
                bytes: limit,
                limit: addr::app_bank_bytes(addr::APP_BASE_LEGACY) as usize,
            })
    );
}

#[test]
fn the_bootloader_must_be_at_the_start_of_flash() {
    let mut bundle = good_bundle();
    bundle.bootloader.load_address = addr::APP_BASE;

    assert!(bundle.validate().contains(&BundleFault::WrongLoadAddress {
        region: RegionName::Bootloader,
        expected: addr::FLASH_BASE,
        found: addr::APP_BASE,
    }));
}

/// There are exactly two application bases. A third means nothing on the board knows how to start
/// the image, so it is refused rather than programmed somewhere plausible-looking.
#[test]
fn an_application_at_neither_base_is_refused() {
    let mut bundle = good_bundle();
    bundle.application.load_address = 0x0800_5000;

    assert!(
        bundle
            .validate()
            .contains(&BundleFault::UnknownApplicationBase { found: 0x0800_5000 })
    );
}

/// Legal, and worth saying out loud: this is the state a board is deliberately left in halfway
/// through a bootloader replacement.
#[test]
fn a_legacy_application_on_a_v6_bootloader_is_a_warning_not_a_refusal() {
    let bundle = bundle(bootloader_bytes(6, 14_000), addr::APP_BASE_LEGACY);
    assert_eq!(
        bundle.validate(),
        vec![],
        "a v6 bootloader starts an image at 0x08006000 when the new bank is blank"
    );
    assert!(
        bundle
            .warnings()
            .contains(&BundleFault::LegacyApplicationOnNewBootloader),
        "it wastes the 8 kB the smaller bootloader freed, which the operator should know"
    );
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
    // And it follows the image rather than the map: a board running a legacy-base application
    // reports 0x08006000, and a run-check that insisted on 0x08004000 would fail a board that is
    // working perfectly.
    assert_eq!(
        legacy_bundle().run_check.vtor,
        addr::APP_BASE_LEGACY,
        "the expected VTOR is the base this bundle's application was linked for"
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
fn the_expected_flash_image_stops_before_durable_pages() {
    let bundle = good_bundle();
    let image = bundle.expected_flash_image();

    assert_eq!(image.len(), addr::FIRMWARE_BYTES as usize);
    assert_eq!(&image[..8], &bundle.bootloader.bytes[..8]);

    // The gap between the end of the bootloader and the application bank is erased flash, not
    // stale bytes -- the bounded pass erases every firmware page, so readback expects 0xFF there.
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

// ---------------------------------------------------------------- one bank at a time

/// The blocker that made "bootloader only" unreachable.
///
/// `Discovery::load` filtered `NoVectorTable` and `BadResetVector` out of a bundle with no
/// application, and `ProbeRsRig::flash` -- which re-validates before it touches the probe -- did
/// not. So a bootloader-only bundle loaded cleanly at the bench and came back from the rig as
/// `BadBundle: application is too short to contain a vector table`, after the operator had pressed
/// Flash. The check now asks the question it always meant: a bank with no bytes has no vector
/// table to be wrong about.
#[test]
fn a_bundle_with_no_application_is_valid_and_needs_no_vector_table() {
    let mut bundle = good_bundle();
    bundle.application = Region::new(RegionName::Application, addr::APP_BASE, Vec::new());

    assert_eq!(
        bundle.validate(),
        vec![],
        "a bootloader-only bundle must be flashable"
    );
    assert_eq!(bundle.scope(), "bootloader only");

    // And the reverse: a bank with bytes is still held to the same standard as before.
    let mut nonsense = good_bundle();
    nonsense.application = Region::new(RegionName::Application, addr::APP_BASE, vec![0u8; 4]);
    assert_eq!(nonsense.validate(), vec![BundleFault::NoVectorTable]);
}

#[test]
fn a_bundle_with_no_bootloader_is_valid_and_is_named_application_only() {
    let mut bundle = good_bundle();
    bundle.bootloader = Region::new(RegionName::Bootloader, addr::FLASH_BASE, Vec::new());

    assert_eq!(bundle.validate(), vec![]);
    assert_eq!(bundle.scope(), "application only");
}

/// The one property that stops "preserve" from destroying the thing it preserves.
///
/// probe-rs pads a partially filled page with the erased byte value, so a window ending mid-page
/// would erase the rest of that page. Every window here starts and ends on a bank boundary, and
/// every bank boundary is a multiple of the 2 kB sector -- which is what makes the padding
/// impossible rather than merely unlikely.
#[test]
fn write_windows_are_page_aligned_and_stop_below_the_durable_pages() {
    let page = u64::from(addr::FLASH_PAGE_BYTES);
    for unselected in [
        portal_swd::Unselected::Preserve,
        portal_swd::Unselected::Erase,
    ] {
        for (boot, app) in [(true, true), (true, false), (false, true)] {
            let mut bundle = good_bundle();
            bundle.unselected = unselected;
            if !boot {
                bundle.bootloader =
                    Region::new(RegionName::Bootloader, addr::FLASH_BASE, Vec::new());
            }
            if !app {
                bundle.application =
                    Region::new(RegionName::Application, addr::APP_BASE, Vec::new());
            }

            for window in bundle.write_windows() {
                assert_eq!(
                    u64::from(window.start) % page,
                    0,
                    "{unselected:?} {boot}/{app}: window starts mid-page at {:#010X}",
                    window.start
                );
                assert_eq!(
                    u64::from(window.end()) % page,
                    0,
                    "{unselected:?} {boot}/{app}: window ends mid-page at {:#010X}",
                    window.end()
                );
                assert!(
                    window.end() <= addr::PERSIST_BASE,
                    "{unselected:?} {boot}/{app}: window reaches the durable pages"
                );
            }
        }
    }
}

/// Preserve and erase must be indistinguishable when both banks are supplied.
///
/// This is what keeps the production pass out of the blast radius: whatever the toggle says, a
/// full selection stages the same bytes over the same span it always did.
#[test]
fn a_full_selection_stages_the_same_bytes_under_either_policy() {
    let mut preserve = good_bundle();
    preserve.unselected = portal_swd::Unselected::Preserve;
    let mut erase = good_bundle();
    erase.unselected = portal_swd::Unselected::Erase;

    let flatten = |bundle: &ImageBundle| {
        let mut out = vec![0xFF_u8; addr::FIRMWARE_BYTES as usize];
        for window in bundle.write_windows() {
            let at = (window.start - addr::FLASH_BASE) as usize;
            out[at..at + window.bytes.len()].copy_from_slice(&window.bytes);
        }
        out
    };
    assert_eq!(flatten(&preserve), flatten(&erase));
    assert_eq!(flatten(&erase), erase.expected_firmware_image());
    assert!(preserve.preserved_windows().is_empty());
    assert!(erase.preserved_windows().is_empty());
}

/// A bank left out is either written as `0xFF` or not written at all, and the two are named
/// separately so nothing downstream has to infer which happened.
#[test]
fn the_policy_decides_whether_a_bank_left_out_is_a_window_or_a_promise() {
    let mut bundle = good_bundle();
    bundle.application = Region::new(RegionName::Application, addr::APP_BASE, Vec::new());

    bundle.unselected = portal_swd::Unselected::Preserve;
    let windows = bundle.write_windows();
    assert_eq!(windows.len(), 1, "only the bootloader bank is written");
    assert_eq!(windows[0].start, addr::FLASH_BASE);
    assert_eq!(windows[0].end(), addr::APP_BASE);
    assert_eq!(
        bundle.preserved_windows(),
        vec![(addr::APP_BASE, addr::PERSIST_BASE)],
        "the application bank is promised, and therefore has to be proved"
    );
    // The bootloader's own bank is padded to its end, so a shorter image cannot leave a tail.
    assert!(windows[0].bytes[14_000..].iter().all(|&b| b == 0xFF));

    bundle.unselected = portal_swd::Unselected::Erase;
    let windows = bundle.write_windows();
    assert_eq!(windows.len(), 1, "the whole partition, as it always was");
    assert_eq!(windows[0].start, addr::FLASH_BASE);
    assert_eq!(windows[0].end(), addr::PERSIST_BASE);
    assert!(
        bundle.preserved_windows().is_empty(),
        "nothing is promised when everything is written"
    );
}
