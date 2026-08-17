//! Finding the firmware this repository can actually flash.
//!
//! # Only `application_bank` is ever offered for the application
//!
//! `PortalFW` builds three environments. `application_bank` links at `0x08006000` via
//! `set_bank2.py`; `no_bootloader` and `debug_no_bootloader` link at `0x08000000`. The second two
//! produce a binary that programs cleanly into the application slot, verifies cleanly, and never
//! runs — the board on the bench during development turned out to be exactly that. So they are
//! refused **by name here** and again **by reset vector** in [`ImageBundle::validate`]; one check
//! is a policy and two are a guarantee.
//!
//! # Paths are resolved from the source tree, not the working directory
//!
//! The same trick `av-operator-app`'s `web_assets!` uses: `CARGO_MANIFEST_DIR` is baked in at
//! compile time, so moving `target/` is harmless and only moving the *source* breaks it. A
//! missing `.pio` is a first-class state with a build hint attached, not an error — the firmware
//! has never been built on a fresh clone, and telling someone to run `pio run` is more use than
//! an empty list.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::addr;
use crate::image::RegionName;

/// Where an artefact came from, and how much it should be trusted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// Built from this repository by PlatformIO.
    Built,
    /// A binary committed to the repository whose source is not (yet) here.
    Reference,
}

/// One flashable file that exists right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Artefact {
    /// Stable across rescans, and what the page writes to select one.
    pub id: String,
    pub label: String,
    pub region: RegionName,
    pub origin: Origin,
    pub path: PathBuf,
    pub bytes: u64,
    /// Seconds since the epoch, or `None` if the filesystem would not say.
    pub modified: Option<u64>,
    /// The ELF beside it, when there is one. Needed later to resolve a liveness symbol.
    pub elf: Option<PathBuf>,
}

impl Artefact {
    pub fn load_address(&self) -> u32 {
        match self.region {
            RegionName::Bootloader => addr::FLASH_BASE,
            RegionName::Application => addr::APP_BASE,
        }
    }

    /// Whether it can physically fit where it is meant to go. Checked here so a too-large file is
    /// visible in the picker rather than at the moment someone presses Flash.
    pub fn fits(&self) -> bool {
        let limit = match self.region {
            RegionName::Bootloader => u64::from(addr::BOOTLOADER_BYTES),
            RegionName::Application => u64::from(addr::APP_BANK_BYTES),
        };
        self.bytes > 0 && self.bytes <= limit
    }
}

/// Somewhere an artefact was expected but is not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Missing {
    pub label: String,
    pub path: PathBuf,
    /// What to do about it, in words an operator can act on.
    pub hint: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Discovery {
    pub found: Vec<Artefact>,
    pub missing: Vec<Missing>,
    /// The repository root everything was resolved against, for the log and for a diagnostic when
    /// somebody has moved the tree.
    pub root: Option<PathBuf>,
}

impl Discovery {
    pub fn application(&self) -> Option<&Artefact> {
        self.found
            .iter()
            .find(|a| a.region == RegionName::Application)
    }

    /// A built bootloader if there is one, otherwise the committed reference. Preferring the
    /// built one is what makes the reference fade out on its own once the port lands.
    pub fn bootloader(&self) -> Option<&Artefact> {
        let boots = || {
            self.found
                .iter()
                .filter(|a| a.region == RegionName::Bootloader)
        };
        boots()
            .find(|a| a.origin == Origin::Built)
            .or_else(|| boots().next())
    }

    pub fn by_id(&self, id: &str) -> Option<&Artefact> {
        self.found.iter().find(|a| a.id == id)
    }
}

/// The repository root, as baked in at compile time.
///
/// `crates/portal-flasher` → up three → `KC79.Reworld`. Canonicalised with a fallback, so a tree
/// that has been moved gives a wrong-looking path in a message rather than a panic.
pub fn repo_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest.join("..").join("..").join("..");
    root.canonicalize().unwrap_or(root)
}

/// What is available to flash, right now.
pub fn discover() -> Discovery {
    discover_in(&repo_root())
}

/// The same, rooted anywhere — which is what makes it testable without a built firmware tree.
pub fn discover_in(root: &Path) -> Discovery {
    let mut found = Vec::new();
    let mut missing = Vec::new();

    // ---- the application
    let app_dir = root.join("PortalFW/.pio/build/application_bank");
    let app_bin = app_dir.join("firmware.bin");
    match stat(&app_bin) {
        Some((bytes, modified)) => found.push(Artefact {
            id: "portalfw:application_bank".into(),
            label: "PortalFW application".into(),
            region: RegionName::Application,
            origin: Origin::Built,
            path: app_bin,
            bytes,
            modified,
            elf: exists(app_dir.join("firmware.elf")),
        }),
        None => missing.push(Missing {
            label: "PortalFW application".into(),
            path: app_bin,
            hint: "not built yet — run `pio run -e application_bank` in PortalFW".into(),
        }),
    }

    // ---- the bootloader, built
    let boot_dir = root.join("PortalBootloader/.pio/build/bootloader");
    let boot_bin = boot_dir.join("firmware.bin");
    match stat(&boot_bin) {
        Some((bytes, modified)) => found.push(Artefact {
            id: "portalbootloader:bootloader".into(),
            label: "PortalBootloader (built)".into(),
            region: RegionName::Bootloader,
            origin: Origin::Built,
            path: boot_bin,
            bytes,
            modified,
            elf: exists(boot_dir.join("firmware.elf")),
        }),
        None => missing.push(Missing {
            label: "PortalBootloader (built)".into(),
            path: boot_bin,
            hint: "the PlatformIO port does not exist yet — the reference image is used instead"
                .into(),
        }),
    }

    // ---- the bootloader, committed reference
    let reference_dir = root.join("PortalBootloader/reference");
    if let Some(reference) = newest_bin(&reference_dir)
        && let Some((bytes, modified)) = stat(&reference)
    {
        found.push(Artefact {
            id: format!(
                "reference:{}",
                reference
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("bootloader")
            ),
            label: "Reference bootloader".into(),
            region: RegionName::Bootloader,
            origin: Origin::Reference,
            path: reference,
            bytes,
            modified,
            elf: None,
        });
    }

    Discovery {
        found,
        missing,
        root: Some(root.to_path_buf()),
    }
}

/// The environments that must never be offered as an application, and why.
///
/// Kept as data so the reason is greppable from the one place that would otherwise silently
/// accept them.
pub const REFUSED_APPLICATION_ENVS: &[(&str, &str)] = &[
    (
        "no_bootloader",
        "links at 0x08000000, so it programs and verifies cleanly into the application slot and \
         never runs",
    ),
    (
        "debug_no_bootloader",
        "links at 0x08000000, and is a debug build that does not fit alongside a bootloader",
    ),
];

/// Whether a PlatformIO environment may supply the application region.
pub fn env_supplies_application(env: &str) -> bool {
    env == "application_bank"
}

fn stat(path: &Path) -> Option<(u64, Option<u64>)> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs());
    Some((meta.len(), modified))
}

fn exists(path: PathBuf) -> Option<PathBuf> {
    path.is_file().then_some(path)
}

/// The most recently modified `.bin` in a directory. More than one reference image is expected
/// eventually — the README asks for a new one to be added beside the old rather than replacing
/// it — so picking deterministically matters.
fn newest_bin(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(u64, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("bin") {
            continue;
        }
        let Some((_, modified)) = stat(&path) else {
            continue;
        };
        let stamp = modified.unwrap_or(0);
        if best.as_ref().is_none_or(|(seen, _)| stamp > *seen) {
            best = Some((stamp, path));
        }
    }
    best.map(|(_, path)| path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("portal-artefacts-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(root: &Path, rel: &str, bytes: usize) -> PathBuf {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, vec![0xA5; bytes]).unwrap();
        path
    }

    #[test]
    fn a_tree_that_has_never_been_built_says_how_to_build_it() {
        // The state of a fresh clone, and of this machine until PortalFW is built for the first
        // time. An empty list would be true and useless.
        let root = scratch("empty");
        let found = discover_in(&root);
        assert!(found.found.is_empty());
        assert!(
            found
                .missing
                .iter()
                .any(|m| m.hint.contains("pio run -e application_bank")),
            "a missing application should carry the command that produces it"
        );
    }

    #[test]
    fn a_built_application_is_found_with_its_elf() {
        let root = scratch("app");
        write(
            &root,
            "PortalFW/.pio/build/application_bank/firmware.bin",
            60_000,
        );
        write(
            &root,
            "PortalFW/.pio/build/application_bank/firmware.elf",
            120_000,
        );

        let found = discover_in(&root);
        let app = found.application().expect("application");
        assert_eq!(app.region, RegionName::Application);
        assert_eq!(app.origin, Origin::Built);
        assert_eq!(app.load_address(), addr::APP_BASE);
        assert_eq!(app.bytes, 60_000);
        assert!(
            app.elf.is_some(),
            "the ELF beside it is what a liveness symbol comes from"
        );
        assert!(app.fits());
    }

    #[test]
    fn the_reference_bootloader_is_offered_when_nothing_is_built() {
        let root = scratch("reference");
        write(
            &root,
            "PortalBootloader/reference/BootloaderRS485-2023-08-26.bin",
            22_708,
        );

        let found = discover_in(&root);
        let boot = found.bootloader().expect("bootloader");
        assert_eq!(boot.origin, Origin::Reference);
        assert_eq!(boot.load_address(), addr::FLASH_BASE);
        assert!(boot.fits(), "22,708 bytes fits the 24 kB bank");
    }

    #[test]
    fn a_built_bootloader_wins_over_the_reference() {
        // Which is what makes the committed binary fade out on its own once the port lands,
        // rather than needing anyone to remember to stop using it.
        let root = scratch("both");
        write(
            &root,
            "PortalBootloader/reference/BootloaderRS485-2023-08-26.bin",
            22_708,
        );
        write(
            &root,
            "PortalBootloader/.pio/build/bootloader/firmware.bin",
            22_000,
        );

        let found = discover_in(&root);
        assert_eq!(found.bootloader().unwrap().origin, Origin::Built);
        assert_eq!(
            found
                .found
                .iter()
                .filter(|a| a.region == RegionName::Bootloader)
                .count(),
            2,
            "both are still offered; only the default changes"
        );
    }

    #[test]
    fn an_oversized_bootloader_is_visible_as_not_fitting() {
        // The CubeIDE linker script says FLASH LENGTH = 28K for a 24 kB bank, so an oversized
        // build links cleanly and overlaps the application. Better seen in the picker than at the
        // moment someone presses Flash.
        let root = scratch("toobig");
        write(&root, "PortalBootloader/reference/big.bin", 26_000);
        let found = discover_in(&root);
        assert!(!found.bootloader().unwrap().fits());
    }

    #[test]
    fn only_application_bank_may_supply_the_application() {
        assert!(env_supplies_application("application_bank"));
        for (env, reason) in REFUSED_APPLICATION_ENVS {
            assert!(!env_supplies_application(env), "{env} must be refused");
            assert!(!reason.is_empty(), "{env} must say why");
        }
    }

    #[test]
    fn the_newest_reference_wins_when_there_is_more_than_one() {
        let root = scratch("two-refs");
        write(&root, "PortalBootloader/reference/old.bin", 22_000);
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let newer = write(&root, "PortalBootloader/reference/new.bin", 22_100);

        let found = discover_in(&root);
        assert_eq!(found.bootloader().unwrap().path, newer);
    }

    /// Against the real repository, not a scratch directory.
    ///
    /// The reference bootloader is committed, so this is stable — and it is the gate that catches
    /// someone deleting it, moving it, or replacing it with something that would not fit the
    /// bank. Skipped rather than failed if the tree has been moved, since `repo_root` is a
    /// compile-time path and a relocated checkout is a wrong answer rather than a broken one.
    #[test]
    fn the_committed_reference_bootloader_is_discoverable() {
        let root = repo_root();
        if !root.join("PortalBootloader/reference").is_dir() {
            eprintln!("skipping: {} is not this repository", root.display());
            return;
        }
        let found = discover_in(&root);
        let boot = found
            .bootloader()
            .expect("the committed reference bootloader should be discoverable");
        assert_eq!(boot.origin, Origin::Reference);
        assert_eq!(boot.bytes, 22_708, "the reference image is 22,708 bytes");
        assert!(boot.fits(), "it must fit the 24 kB bootloader bank");
    }

    #[test]
    fn a_directory_is_not_mistaken_for_a_binary() {
        let root = scratch("dir");
        std::fs::create_dir_all(root.join("PortalFW/.pio/build/application_bank/firmware.bin"))
            .unwrap();
        assert!(discover_in(&root).application().is_none());
    }
}

// ---------------------------------------------------------------- loading

use crate::image::{ImageBundle, OptionBytePolicy, Provenance, Region, RunCheckSpec};

/// Why a selection could not be turned into something flashable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// Neither region was chosen. Flashing nothing is not a scope.
    NothingSelected,
    UnknownArtefact(String),
    Unreadable {
        path: PathBuf,
        reason: String,
    },
    /// The bytes are real but the bundle they make is not valid — wrong load address, an image
    /// that would overlap the other bank, a reset vector pointing at the wrong place.
    Invalid(Vec<crate::image::BundleFault>),
}

impl core::fmt::Display for LoadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            LoadError::NothingSelected => {
                f.write_str("choose a bootloader, an application, or both")
            }
            LoadError::UnknownArtefact(id) => write!(f, "no artefact called {id:?}"),
            LoadError::Unreadable { path, reason } => {
                write!(f, "could not read {}: {reason}", path.display())
            }
            LoadError::Invalid(faults) => {
                let joined = faults
                    .iter()
                    .map(|fault| fault.to_string())
                    .collect::<Vec<_>>()
                    .join("; ");
                write!(f, "{joined}")
            }
        }
    }
}

/// What the operator picked: either region, both, or neither.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub bootloader: Option<String>,
    pub application: Option<String>,
}

impl Selection {
    /// What this selection would actually write, in the words the page uses.
    ///
    /// The scope is not a separate control: it *is* which regions were chosen, so the two can
    /// never disagree.
    pub fn scope(&self) -> &'static str {
        match (self.bootloader.is_some(), self.application.is_some()) {
            (true, true) => "full",
            (true, false) => "bootloader only",
            (false, true) => "application only",
            (false, false) => "nothing",
        }
    }
}

impl Discovery {
    /// Turn a selection into something that can be flashed.
    ///
    /// An unselected region is left **erased** rather than preserved: a pass mass-erases before
    /// programming, so anything not supplied genuinely will be `0xFF` afterwards, and an image
    /// that claimed otherwise would make the map lie.
    pub fn load(&self, selection: &Selection) -> Result<ImageBundle, LoadError> {
        if selection.bootloader.is_none() && selection.application.is_none() {
            return Err(LoadError::NothingSelected);
        }

        let read = |id: &Option<String>| -> Result<(Vec<u8>, String), LoadError> {
            let Some(id) = id else {
                return Ok((Vec::new(), "erased".to_owned()));
            };
            let artefact = self
                .by_id(id)
                .ok_or_else(|| LoadError::UnknownArtefact(id.clone()))?;
            let bytes = std::fs::read(&artefact.path).map_err(|err| LoadError::Unreadable {
                path: artefact.path.clone(),
                reason: err.to_string(),
            })?;
            Ok((bytes, artefact.label.clone()))
        };

        let (boot_bytes, boot_from) = read(&selection.bootloader)?;
        let (app_bytes, app_from) = read(&selection.application)?;

        let bundle = ImageBundle {
            bootloader: Region::new(RegionName::Bootloader, addr::FLASH_BASE, boot_bytes),
            application: Region::new(RegionName::Application, addr::APP_BASE, app_bytes),
            option_bytes: OptionBytePolicy::default(),
            // No ELF reader yet, so no liveness symbol. `ImageBundle::warnings` reports that;
            // `validate` deliberately does not refuse it.
            run_check: RunCheckSpec::default(),
            provenance: Provenance::Composed {
                bootloader: boot_from,
                application: app_from,
            },
        };

        // A region that was not selected has no vector table to check; everything else still
        // applies.
        let faults: Vec<_> = bundle
            .validate()
            .into_iter()
            .filter(|fault| {
                // A region that was not selected has no vector table to check.
                !(selection.application.is_none()
                    && matches!(
                        fault,
                        crate::image::BundleFault::BadResetVector { .. }
                            | crate::image::BundleFault::NoVectorTable
                    ))
            })
            .collect();
        if !faults.is_empty() {
            return Err(LoadError::Invalid(faults));
        }
        Ok(bundle)
    }
}

#[cfg(test)]
mod load_tests {
    use super::*;

    /// A minimal application image: a vector table that points into the application bank.
    fn application_bytes(len: usize) -> Vec<u8> {
        let mut bytes = vec![0u8; len.max(8)];
        bytes[0..4].copy_from_slice(&0x2000_9000u32.to_le_bytes());
        bytes[4..8].copy_from_slice(&(addr::APP_BASE + 0x241).to_le_bytes());
        bytes
    }

    fn tree(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("portal-load-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        let app = dir.join("PortalFW/.pio/build/application_bank");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("firmware.bin"), application_bytes(60_000)).unwrap();
        let reference = dir.join("PortalBootloader/reference");
        std::fs::create_dir_all(&reference).unwrap();
        std::fs::write(reference.join("boot.bin"), vec![0xA5; 22_708]).unwrap();
        dir
    }

    #[test]
    fn both_regions_make_a_full_image() {
        let found = discover_in(&tree("full"));
        let selection = Selection {
            bootloader: found.bootloader().map(|a| a.id.clone()),
            application: found.application().map(|a| a.id.clone()),
        };
        assert_eq!(selection.scope(), "full");

        let bundle = found.load(&selection).expect("full image");
        assert_eq!(bundle.bootloader.bytes.len(), 22_708);
        assert_eq!(bundle.application.bytes.len(), 60_000);
        assert_eq!(bundle.bootloader.load_address, addr::FLASH_BASE);
        assert_eq!(bundle.application.load_address, addr::APP_BASE);
    }

    #[test]
    fn one_region_leaves_the_other_erased_rather_than_preserved() {
        // A pass mass-erases before programming, so anything not supplied genuinely will be 0xFF
        // afterwards. An image that claimed otherwise would make the map lie about the board.
        let found = discover_in(&tree("app-only"));
        let selection = Selection {
            bootloader: None,
            application: found.application().map(|a| a.id.clone()),
        };
        assert_eq!(selection.scope(), "application only");

        let bundle = found.load(&selection).expect("application only");
        assert!(bundle.bootloader.bytes.is_empty());
        let image = bundle.expected_flash_image();
        assert!(
            image[..addr::BOOTLOADER_BYTES as usize]
                .iter()
                .all(|&b| b == 0xFF),
            "the unselected bank must read as erased"
        );
    }

    #[test]
    fn a_bootloader_only_selection_does_not_demand_an_application_vector_table() {
        let found = discover_in(&tree("boot-only"));
        let selection = Selection {
            bootloader: found.bootloader().map(|a| a.id.clone()),
            application: None,
        };
        assert_eq!(selection.scope(), "bootloader only");
        assert!(found.load(&selection).is_ok());
    }

    #[test]
    fn selecting_nothing_is_refused_with_a_usable_message() {
        let found = discover_in(&tree("nothing"));
        let err = found.load(&Selection::default()).unwrap_err();
        assert_eq!(err, LoadError::NothingSelected);
        assert!(err.to_string().contains("choose"));
    }

    #[test]
    fn an_unknown_id_is_refused_by_name() {
        let found = discover_in(&tree("unknown"));
        let err = found
            .load(&Selection {
                application: Some("portalfw:nonexistent".into()),
                ..Selection::default()
            })
            .unwrap_err();
        assert!(matches!(err, LoadError::UnknownArtefact(id) if id.contains("nonexistent")));
    }

    #[test]
    fn an_application_linked_for_the_bootloader_slot_is_refused_on_load() {
        // The mistake that costs a bench session: `pio run -e no_bootloader` links at 0x08000000,
        // and the resulting binary programs and verifies cleanly into the application slot but
        // never runs. Refused by name during discovery, and again here by reset vector.
        let dir = tree("wrong-link");
        let app = dir.join("PortalFW/.pio/build/application_bank/firmware.bin");
        let mut bytes = application_bytes(60_000);
        bytes[4..8].copy_from_slice(&(addr::FLASH_BASE + 0x241).to_le_bytes());
        std::fs::write(&app, bytes).unwrap();

        let found = discover_in(&dir);
        let err = found
            .load(&Selection {
                bootloader: None,
                application: found.application().map(|a| a.id.clone()),
            })
            .unwrap_err();
        assert!(err.to_string().contains("reset vector"), "got: {err}");
    }

    #[test]
    fn a_loaded_bundle_records_where_each_half_came_from() {
        let found = discover_in(&tree("provenance"));
        let bundle = found
            .load(&Selection {
                bootloader: found.bootloader().map(|a| a.id.clone()),
                application: found.application().map(|a| a.id.clone()),
            })
            .unwrap();
        match bundle.provenance {
            crate::image::Provenance::Composed {
                bootloader,
                application,
            } => {
                assert!(bootloader.contains("Reference"));
                assert!(application.contains("PortalFW"));
            }
            other => panic!("expected Composed, got {other:?}"),
        }
    }

    #[test]
    fn a_loaded_bundle_warns_that_it_cannot_be_run_checked_yet() {
        // No ELF reader, so no liveness symbol. Flashable, but not automatically verifiable --
        // and that has to be visible rather than silently absent.
        let found = discover_in(&tree("warn"));
        let bundle = found
            .load(&Selection {
                bootloader: None,
                application: found.application().map(|a| a.id.clone()),
            })
            .unwrap();
        assert_eq!(bundle.validate(), vec![]);
        assert!(
            bundle
                .warnings()
                .contains(&crate::image::BundleFault::NoLivenessAddress)
        );
    }
}
