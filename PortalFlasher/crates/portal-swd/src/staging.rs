//! Firmware an operator hands the application directly, rather than firmware it went looking for.
//!
//! [`crate::artefacts::discover_in`] answers "what has this tree built?" by walking four hardcoded
//! `PortalFW/.pio/build/<env>/firmware.bin` paths and the two bootloader locations. That is the
//! right answer for a bench standing in a checkout, and no answer at all for the ordinary case of
//! being sent one file: a colleague's build, a bisect candidate, a release candidate off CI. The
//! only escape was `PORTAL_FIRMWARE_DIR`, read once at startup, so using it meant quitting.
//!
//! This module is the other door. Bytes arrive, they are identified, they are written into a
//! per-user staging directory, and from there they are ordinary [`Artefact`]s -- the same struct,
//! the same `Discovery::load`, the same `ImageBundle::validate`, the same rig. Nothing downstream
//! learns that a file came in this way except through [`Origin::Dropped`], which exists so the
//! *page* can say so.
//!
//! # Identifying a file is a question this crate had never been asked
//!
//! Everywhere else, which bank an image belongs to is decided by *where it was found*
//! (`PortalFW/.pio/build/...` is an application, `PortalBootloader/...` is a bootloader) or by
//! *which subcommand was typed* (`flash_portals app` against `flash_portals bootloader`). Handed a
//! bare buffer, neither is available, so [`classify`] is a new thing and it is deliberately built
//! out of the positive tests that already exist rather than a new set of guesses:
//!
//! - [`crate::image::image_base`] is the application test. It reads the `KC79APP1` descriptor at
//!   `base + 0xC0`, and only falls back to the legacy base when a descriptor-less image's reset
//!   vector proves it. It is also the test that refuses a `no_bootloader` build, which is the one
//!   image that would program cleanly, verify cleanly and never run.
//! - [`crate::device::bootloader_version`] is the bootloader test. The `"Bootloader v"` banner is
//!   the *only* positive bootloader identifier there is -- `PortalBootloader/tools/size_gate.py`
//!   fails the build if the linker optimises it away, precisely so this remains true -- and
//!   `router_link::bootloader_update::validate` already refuses on it for the same reason.
//!
//! Classification decides **routing**: which bank, and which base. It is not the authority on
//! whether an image may be written. That stays where it was, in `ImageBundle::validate`, which
//! runs on every load and again inside the rig before the probe is touched. A file that classifies
//! and then fails validation is a file that gets listed and refused by name, which is what an
//! operator staring at a build they can see needs -- the same reasoning `discover_in` applies to a
//! too-large `.bin`.
//!
//! # Why the id is a content hash
//!
//! `Selection` holds artefact ids, and `rescan` re-fills a bank whose id has vanished. An id
//! derived from the filename would collide the moment two `firmware.bin`s were dropped, and an id
//! derived from a counter would move across a restart and silently deselect. The first twelve hex
//! digits of the SHA-256 do neither: dropping the same bytes twice is idempotent, dropping two
//! different builds called `firmware.bin` gives two rows, and the id an operator selected before
//! lunch still means the same image after.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::artefacts::{Artefact, Origin};
use crate::image::{BaseSource, RegionName};
use crate::{addr, device, elf, image};

/// How many staged images to keep. Old enough to cover a day's work, small enough that the picker
/// does not become a file browser.
pub const KEEP: usize = 12;

/// What a buffer turned out to be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Classification {
    Application {
        base: u32,
        source: BaseSource,
    },
    Bootloader {
        version: Option<u32>,
    },
    /// Identifiable as neither, with the reason in the operator's words.
    Refused(Refusal),
}

impl Classification {
    pub fn region(&self) -> Option<RegionName> {
        match self {
            Classification::Application { .. } => Some(RegionName::Application),
            Classification::Bootloader { .. } => Some(RegionName::Bootloader),
            Classification::Refused(_) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Shorter than a vector table.
    TooShort,
    /// Larger than the part's whole flash.
    TooLarge { bytes: usize },
    /// An ELF that could not be turned into an image.
    Elf(elf::ElfError),
    /// A `no_bootloader` build: linked at `0x08000000`, so it carries no descriptor and no
    /// bootloader banner. It would program and verify and never run.
    NoBootloaderBuild,
    /// A plausible Cortex-M image that answers neither positive test.
    Unidentified,
}

impl core::fmt::Display for Refusal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Refusal::TooShort => write!(f, "too short to be a firmware image"),
            Refusal::TooLarge { bytes } => write!(
                f,
                "{} kB is larger than this part's {} kB of flash",
                bytes / 1024,
                (addr::FLASH_END - addr::FLASH_BASE) / 1024
            ),
            Refusal::Elf(error) => write!(f, "{error}"),
            Refusal::NoBootloaderBuild => write!(
                f,
                "this looks like a `no_bootloader` build -- linked at {:#010X}, it would program \
                 and verify and never run. Flash an application built for {:#010X} or {:#010X}",
                addr::FLASH_BASE,
                addr::APP_BASE,
                addr::APP_BASE_LEGACY
            ),
            Refusal::Unidentified => write!(
                f,
                "not recognised as either bank: no `KC79APP1` application descriptor and no \
                 `Bootloader v` banner"
            ),
        }
    }
}

/// What a buffer is, from the bytes alone.
///
/// The application test runs first because it is the stronger one: a descriptor states its base
/// where the banner only names a product. The two cannot both fire on a real image -- an
/// application carries no bootloader banner and a bootloader carries no descriptor -- so the order
/// is about which answer to trust if a file ever managed both, not about which is likely.
pub fn classify(bytes: &[u8]) -> Classification {
    if bytes.len() < 8 {
        return Classification::Refused(Refusal::TooShort);
    }
    if bytes.len() > (addr::FLASH_END - addr::FLASH_BASE) as usize {
        return Classification::Refused(Refusal::TooLarge { bytes: bytes.len() });
    }
    if let Some((base, source)) = image::image_base(bytes) {
        return Classification::Application { base, source };
    }
    if let Some(version) = bootloader_shaped(bytes) {
        return Classification::Bootloader { version };
    }
    // Distinguish the one failure worth naming from the general one. A `no_bootloader` build has a
    // perfectly good vector table whose reset vector lands in the bootloader's own bank, and no
    // banner -- which is exactly what this checks, and exactly why `image_base` returned `None`.
    match image::vector_table(bytes) {
        Some((_, reset))
            if reset & 1 == 1
                && (addr::FLASH_BASE..addr::APP_BASE).contains(&(reset & !1)) =>
        {
            Classification::Refused(Refusal::NoBootloaderBuild)
        }
        _ => Classification::Refused(Refusal::Unidentified),
    }
}

/// A bootloader, by its banner and its vector table.
///
/// The banner alone is not enough: an application that happened to contain the string -- a log
/// format, a version table -- would be routed into the bootloader bank and overwrite the one thing
/// a board cannot recover without. So the vector table has to agree: a stack pointer in SRAM, and
/// a Thumb reset vector inside the larger of the two bootloader banks. Those are the same three
/// checks `router_link::bootloader_update::validate` applies before it will send an image over
/// RS485.
fn bootloader_shaped(bytes: &[u8]) -> Option<Option<u32>> {
    let (sp, reset) = image::vector_table(bytes)?;
    let entry = reset & !1;
    let plausible = (addr::RAM_BASE..=addr::RAM_END).contains(&sp)
        && reset & 1 == 1
        && (addr::FLASH_BASE..addr::FLASH_BASE + addr::BOOTLOADER_BYTES_LEGACY).contains(&entry);
    if !plausible {
        return None;
    }
    device::bootloader_version(bytes).map(Some)
}

// ------------------------------------------------------------------ staging

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StageError {
    /// The bytes are not something this can flash. Nothing was written.
    Refused(Refusal),
    Io { path: PathBuf, detail: String },
}

impl core::fmt::Display for StageError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            StageError::Refused(refusal) => write!(f, "{refusal}"),
            StageError::Io { path, detail } => {
                write!(f, "could not write {}: {detail}", path.display())
            }
        }
    }
}

/// The record written beside each staged image.
///
/// It exists so [`discover`] can name a row the way the operator does -- by the file they dropped
/// -- rather than by the content hash the file on disk is named after. Everything else about the
/// artefact (banner, base, size) is re-read from the bytes, because the bytes are the truth and a
/// sidecar that disagreed with them would be a second source to keep in step.
///
/// Note what it does *not* add to [`Artefact`]: the filename becomes the artefact's `label` and
/// the drop time becomes its `modified`, which are the two fields those facts already mean.
/// `Origin::Dropped` therefore stays a unit variant and `Origin` stays `Copy`.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Sidecar {
    /// The operator's own filename, as dropped.
    pub name: String,
    /// Seconds since the epoch.
    pub dropped_at: u64,
    /// Which bank this was staged into. Recorded rather than re-derived so that a bank forced by
    /// the operator -- dropping on a specific target -- survives a restart.
    pub region: String,
    /// `true` when the drop was an ELF and the `.bin` beside this is the flattened image.
    pub from_elf: bool,
}

/// Which bank an operator asked for, when they asked.
///
/// `Auto` is the ordinary case and lets [`classify`] decide. The two explicit arms exist for the
/// gesture of dropping onto a named bank, which is how an operator overrides a file this cannot
/// identify -- and they are still checked: an explicit bank that contradicts a *confident*
/// classification is refused rather than obeyed, because "I meant the bootloader bank" is not a
/// reason to write an application image over a board's only way to boot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bank {
    Auto,
    Bootloader,
    Application,
}

/// Write one dropped file into the staging directory and return it as an artefact.
///
/// An ELF is flattened first ([`crate::elf::flatten`]) and kept alongside the flat image: the ELF
/// is the only thing that carries `g_liveness_counter`, so a dropped ELF gets a real run-check
/// where a dropped `.bin` structurally cannot.
///
/// Nothing is written for a file that classifies as refused. A staging directory that accumulated
/// unflashables would put them in the picker, where the only thing to do with them is read the
/// refusal again.
pub fn stage(dir: &Path, name: &str, bytes: &[u8], bank: Bank) -> Result<Artefact, StageError> {
    let name = Path::new(name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("dropped.bin")
        .to_owned();

    let (image, source_elf) = if elf::is_elf(bytes) {
        let flat = elf::flatten(bytes).map_err(|error| StageError::Refused(Refusal::Elf(error)))?;
        (flat.bytes, Some(bytes))
    } else {
        (bytes.to_vec(), None)
    };

    let region = match (classify(&image), bank) {
        (Classification::Refused(refusal), Bank::Auto) => {
            return Err(StageError::Refused(refusal));
        }
        // An unidentified image the operator has aimed at a bank is taken at their word: they have
        // named the bank, `validate` still has to agree, and refusing here would leave them with a
        // file the bench can see and nothing to do about it.
        (Classification::Refused(_), Bank::Bootloader) => RegionName::Bootloader,
        (Classification::Refused(_), Bank::Application) => RegionName::Application,
        // A confident classification wins over the gesture. Dropping an application on the
        // bootloader target is a slip, and obeying it writes over the one bank a board cannot
        // recover from on its own.
        (known, _) => known.region().expect("a non-refusal names its region"),
    };

    let id = short_hash(&image);
    std::fs::create_dir_all(dir).map_err(|err| StageError::Io {
        path: dir.to_path_buf(),
        detail: err.to_string(),
    })?;
    let bin = dir.join(format!("{id}.bin"));
    write(&bin, &image)?;
    if let Some(elf_bytes) = source_elf {
        write(&dir.join(format!("{id}.elf")), elf_bytes)?;
    }
    let sidecar = Sidecar {
        name,
        dropped_at: now(),
        region: region.as_str().to_owned(),
        from_elf: source_elf.is_some(),
    };
    write(
        &dir.join(format!("{id}.json")),
        serde_json::to_string_pretty(&sidecar)
            .unwrap_or_default()
            .as_bytes(),
    )?;

    let staged = read_one(dir, &id).ok_or_else(|| StageError::Io {
        path: bin,
        detail: "the staged image could not be read back".into(),
    })?;
    prune(dir, &id);
    Ok(staged)
}

/// Every staged image, newest first.
///
/// Rebuilt from the files each time rather than cached: this runs on a rescan and on adoption,
/// the images are tens of kilobytes, and `discover_in` already reads every artefact whole to
/// scrape its banner. A cache would be a second truth for no measurable gain.
pub fn discover(dir: &Path) -> Vec<Artefact> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<Artefact> = entries
        .flatten()
        .filter(|entry| entry.path().extension().and_then(|e| e.to_str()) == Some("json"))
        .filter_map(|entry| {
            let id = entry.path().file_stem()?.to_str()?.to_owned();
            read_one(dir, &id)
        })
        .collect();
    found.sort_by(|a, b| b.modified.cmp(&a.modified).then(a.id.cmp(&b.id)));
    found
}

/// Remove one staged image and everything written with it.
pub fn remove(dir: &Path, id: &str) -> bool {
    let Some(hash) = id.strip_prefix("dropped:") else {
        return false;
    };
    // The id is a content hash this module minted, so it is hex -- but it arrives over HTTP, and a
    // path built from an unchecked string is how a delete route becomes something else entirely.
    if hash.is_empty() || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return false;
    }
    let mut removed = false;
    for extension in ["bin", "elf", "json"] {
        removed |= std::fs::remove_file(dir.join(format!("{hash}.{extension}"))).is_ok();
    }
    removed
}

/// Rebuild one artefact from its staged files.
fn read_one(dir: &Path, id: &str) -> Option<Artefact> {
    let bin = dir.join(format!("{id}.bin"));
    let meta = std::fs::metadata(&bin).ok()?;
    if !meta.is_file() {
        return None;
    }
    let sidecar: Sidecar = serde_json::from_slice(&std::fs::read(dir.join(format!("{id}.json"))).ok()?).ok()?;
    let bytes = std::fs::read(&bin).ok()?;
    let region = match sidecar.region.as_str() {
        "bootloader" => RegionName::Bootloader,
        _ => RegionName::Application,
    };
    // The base is re-read from the image, never from the sidecar: it is the one field where being
    // wrong produces a board that programs, verifies and hard-faults.
    let (base, base_source) = match region {
        RegionName::Bootloader => (addr::FLASH_BASE, None),
        RegionName::Application => match image::image_base(&bytes) {
            Some((base, source)) => (base, Some(source)),
            None => (addr::APP_BASE_LEGACY, None),
        },
    };
    let elf = dir.join(format!("{id}.elf"));
    Some(Artefact {
        id: format!("dropped:{id}"),
        label: sidecar.name.clone(),
        region,
        origin: Origin::Dropped,
        path: bin,
        bytes: meta.len(),
        modified: Some(sidecar.dropped_at),
        elf: elf.is_file().then_some(elf),
        variant: None,
        hardware: None,
        banner: device::first_banner(&bytes),
        base,
        base_source,
    })
}

/// Keep the newest [`KEEP`] and delete the rest.
///
/// `keep` is the image this call just staged, and it is held out of the pruning unconditionally.
/// Without that it can be the one deleted: `modified` has one-second resolution, so a dozen drops
/// in quick succession all carry the same stamp, the sort falls through to the id -- a content
/// hash, i.e. arbitrary order -- and `stage` returns an artefact whose file it has already
/// removed. That is what this test caught.
fn prune(dir: &Path, keep: &str) {
    let keep = format!("dropped:{keep}");
    // The fresh image takes one of the slots, so the rest of the store gets `KEEP - 1`.
    let mut budget = KEEP.saturating_sub(1);
    for artefact in discover(dir) {
        if artefact.id == keep {
            continue;
        }
        if budget > 0 {
            budget -= 1;
            continue;
        }
        remove(dir, &artefact.id);
    }
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), StageError> {
    std::fs::write(path, bytes).map_err(|err| StageError::Io {
        path: path.to_path_buf(),
        detail: err.to_string(),
    })
}

fn short_hash(bytes: &[u8]) -> String {
    device::sha256_hex(bytes)[..12].to_owned()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("portal-staging-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// An application image with a descriptor, the same shape `artefacts`' tests build.
    fn app_image(base: u32, len: usize) -> Vec<u8> {
        let mut bytes = vec![0xA5; len];
        bytes[0..4].copy_from_slice(&0x2000_9000u32.to_le_bytes());
        bytes[4..8].copy_from_slice(&((base + 0x240) | 1).to_le_bytes());
        let at = addr::APP_DESCRIPTOR_OFFSET;
        bytes[at..at + 8].copy_from_slice(addr::APP_DESCRIPTOR_MAGIC);
        bytes[at + 8..at + 12].copy_from_slice(&base.to_le_bytes());
        bytes[at + 12..at + 16].copy_from_slice(&0u32.to_le_bytes());
        bytes[at + 16..at + 16 + addr::APP_VERSION_BYTES].fill(0);
        bytes[at + 16..at + 16 + 8].copy_from_slice(b"Portal v");
        bytes
    }

    /// A bootloader: a vector table into its own bank, and the banner the size gate protects.
    fn bootloader_image(version: u32, len: usize) -> Vec<u8> {
        let mut bytes = vec![0xA5; len];
        bytes[0..4].copy_from_slice(&addr::HANDOFF_ADDR.to_le_bytes());
        bytes[4..8].copy_from_slice(&((addr::FLASH_BASE + 0x200) | 1).to_le_bytes());
        let banner = format!("Bootloader v{version}\0");
        bytes[0x300..0x300 + banner.len()].copy_from_slice(banner.as_bytes());
        bytes
    }

    /// A `no_bootloader` build: linked at `0x08000000`, no descriptor, no banner.
    fn no_bootloader_image(len: usize) -> Vec<u8> {
        let mut bytes = vec![0xA5; len];
        bytes[0..4].copy_from_slice(&0x2000_9000u32.to_le_bytes());
        bytes[4..8].copy_from_slice(&((addr::FLASH_BASE + 0x240) | 1).to_le_bytes());
        bytes
    }

    #[test]
    fn a_descriptor_names_the_application_bank_and_its_base() {
        for base in [addr::APP_BASE, addr::APP_BASE_LEGACY] {
            assert_eq!(
                classify(&app_image(base, 60_000)),
                Classification::Application {
                    base,
                    source: BaseSource::Descriptor,
                },
            );
        }
    }

    #[test]
    fn a_descriptorless_legacy_image_is_still_an_application() {
        let mut bytes = vec![0xA5; 60_000];
        bytes[0..4].copy_from_slice(&0x2000_9000u32.to_le_bytes());
        bytes[4..8].copy_from_slice(&((addr::APP_BASE_LEGACY + 0x240) | 1).to_le_bytes());
        assert_eq!(
            classify(&bytes),
            Classification::Application {
                base: addr::APP_BASE_LEGACY,
                source: BaseSource::InferredLegacy,
            },
        );
    }

    #[test]
    fn a_banner_and_a_vector_table_name_the_bootloader_bank() {
        assert_eq!(
            classify(&bootloader_image(6, 15_000)),
            Classification::Bootloader { version: Some(6) },
        );
    }

    /// The banner on its own must not be enough. An application that happened to carry the string
    /// would otherwise be routed over the one bank a board cannot recover without.
    #[test]
    fn an_application_carrying_the_bootloader_string_is_still_an_application() {
        let mut bytes = app_image(addr::APP_BASE, 60_000);
        bytes[0x400..0x400 + 13].copy_from_slice(b"Bootloader v6");
        assert_eq!(
            classify(&bytes),
            Classification::Application {
                base: addr::APP_BASE,
                source: BaseSource::Descriptor,
            },
        );
    }

    #[test]
    fn a_no_bootloader_build_is_refused_by_name() {
        assert_eq!(
            classify(&no_bootloader_image(60_000)),
            Classification::Refused(Refusal::NoBootloaderBuild),
        );
    }

    #[test]
    fn nonsense_is_refused_rather_than_routed() {
        assert_eq!(
            classify(&[0x00, 0x01]),
            Classification::Refused(Refusal::TooShort)
        );
        assert_eq!(
            classify(&vec![0x00; 200_000]),
            Classification::Refused(Refusal::TooLarge { bytes: 200_000 })
        );
        assert_eq!(
            classify(&vec![0xAA; 4096]),
            Classification::Refused(Refusal::Unidentified)
        );
    }

    #[test]
    fn staging_round_trips_through_discovery() {
        let dir = scratch("round-trip");
        let staged = stage(&dir, "my build.bin", &app_image(addr::APP_BASE, 60_000), Bank::Auto)
            .expect("an application image stages");
        assert_eq!(staged.region, RegionName::Application);
        assert_eq!(staged.origin, Origin::Dropped);
        assert_eq!(staged.base, addr::APP_BASE);
        assert_eq!(staged.label, "my build.bin");
        assert!(staged.fits());

        let found = discover(&dir);
        assert_eq!(found, vec![staged]);
    }

    /// The id is the content hash, so the same bytes twice is one row rather than two.
    #[test]
    fn dropping_the_same_bytes_twice_is_idempotent() {
        let dir = scratch("idempotent");
        let image = app_image(addr::APP_BASE, 60_000);
        let first = stage(&dir, "firmware.bin", &image, Bank::Auto).unwrap();
        let second = stage(&dir, "firmware.bin", &image, Bank::Auto).unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(discover(&dir).len(), 1);
    }

    /// ...and two different builds that share a filename are two rows, which is the case an
    /// id derived from the name would have collapsed into one.
    #[test]
    fn two_builds_called_firmware_bin_are_two_rows() {
        let dir = scratch("two-builds");
        stage(&dir, "firmware.bin", &app_image(addr::APP_BASE, 60_000), Bank::Auto).unwrap();
        stage(&dir, "firmware.bin", &app_image(addr::APP_BASE, 61_000), Bank::Auto).unwrap();
        assert_eq!(discover(&dir).len(), 2);
    }

    #[test]
    fn a_refused_file_writes_nothing() {
        let dir = scratch("refused");
        let error = stage(&dir, "firmware.bin", &no_bootloader_image(60_000), Bank::Auto)
            .expect_err("a no_bootloader build is refused");
        assert_eq!(error, StageError::Refused(Refusal::NoBootloaderBuild));
        assert!(discover(&dir).is_empty());
    }

    /// A confident classification beats the gesture: an application dropped on the bootloader
    /// target lands in the application bank, because the alternative overwrites a board's only
    /// way to boot on a slip of the pointer.
    #[test]
    fn an_explicit_bank_cannot_move_an_identified_image() {
        let dir = scratch("explicit-wrong");
        let staged = stage(
            &dir,
            "firmware.bin",
            &app_image(addr::APP_BASE, 60_000),
            Bank::Bootloader,
        )
        .unwrap();
        assert_eq!(staged.region, RegionName::Application);
    }

    /// But an *unidentified* image aimed at a bank is taken at the operator's word -- `validate`
    /// is still between it and the board.
    #[test]
    fn an_explicit_bank_rescues_an_unidentified_image() {
        let dir = scratch("explicit-rescue");
        let staged = stage(&dir, "mystery.bin", &vec![0xAA; 4096], Bank::Application).unwrap();
        assert_eq!(staged.region, RegionName::Application);
        assert!(stage(&dir, "mystery.bin", &vec![0xAA; 4096], Bank::Auto).is_err());
    }

    /// A dropped ELF is flattened, and the ELF is kept -- which is the whole reason to accept one:
    /// it is the only thing that carries `g_liveness_counter`.
    #[test]
    fn a_dropped_elf_is_flattened_and_kept_beside_its_image() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../PortalBootloader/reference");
        let Ok(elf) = std::fs::read(dir.join("BootloaderRS485-2023-08-26.elf")) else {
            eprintln!("skipped: PortalBootloader/reference is not present");
            return;
        };
        let bin = std::fs::read(dir.join("BootloaderRS485-2023-08-26.bin")).unwrap();
        let staging = scratch("elf");
        let staged = stage(&staging, "BootloaderRS485.elf", &elf, Bank::Auto).unwrap();
        assert_eq!(staged.region, RegionName::Bootloader);
        assert_eq!(staged.base, addr::FLASH_BASE);
        assert_eq!(std::fs::read(&staged.path).unwrap(), bin);
        assert!(staged.elf.is_some(), "the ELF is kept for the run-check");
    }

    #[test]
    fn removing_a_staged_image_takes_its_sidecar_with_it() {
        let dir = scratch("remove");
        let staged = stage(&dir, "firmware.bin", &app_image(addr::APP_BASE, 60_000), Bank::Auto).unwrap();
        assert!(remove(&dir, &staged.id));
        assert!(discover(&dir).is_empty());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    }

    /// The id arrives over HTTP, so a path built from it must not be able to leave the directory.
    #[test]
    fn remove_refuses_anything_that_is_not_one_of_our_hashes() {
        let dir = scratch("traversal");
        stage(&dir, "firmware.bin", &app_image(addr::APP_BASE, 60_000), Bank::Auto).unwrap();
        assert!(!remove(&dir, "dropped:../../etc/passwd"));
        assert!(!remove(&dir, "../../etc/passwd"));
        assert!(!remove(&dir, "dropped:"));
        assert_eq!(discover(&dir).len(), 1);
    }

    #[test]
    fn staging_keeps_only_the_newest_dozen_and_never_prunes_the_fresh_one() {
        let dir = scratch("prune");
        let mut last = None;
        for size in 0..KEEP + 4 {
            last = Some(
                stage(
                    &dir,
                    &format!("build-{size}.bin"),
                    &app_image(addr::APP_BASE, 60_000 + size),
                    Bank::Auto,
                )
                .unwrap(),
            );
        }
        let last = last.unwrap();
        assert_eq!(discover(&dir).len(), KEEP);
        // The one just staged must still be there. All sixteen drops share a one-second `modified`
        // stamp, so the sort falls through to the content hash -- and a prune that did not hold the
        // fresh image out would return an artefact whose file it had already deleted.
        assert!(last.path.is_file());
        assert!(discover(&dir).iter().any(|a| a.id == last.id));
    }
}
