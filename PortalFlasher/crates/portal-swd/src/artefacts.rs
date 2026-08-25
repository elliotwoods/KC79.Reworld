//! Finding the firmware this repository can actually flash.
//!
//! # Two PCB revisions, both offered, neither auto-detected
//!
//! `PortalFW` builds two application environments, one per PCB revision in production:
//! `application_bank_optical` (v6, the reflective-sensor home switch, default) and
//! `application_bank_mechanical` (v4, rev-1 mechanical switches, `-D HOME_SWITCH_LEGACY`). Both
//! link at `0x08006000` via `set_bank2.py` and are offered side by side, the same way a built
//! bootloader and the committed reference are both offered — there is no hardware strap on the
//! board that would let this module tell which revision is attached, so the operator picks by
//! board type rather than the flasher guessing. [`Discovery::application`] still gives a default
//! (optical) for the common case; it is a starting point, not a substitute for the choice.
//!
//! `no_bootloader` and `debug_no_bootloader` link at `0x08000000` instead of `0x08006000`. Either
//! produces a binary that programs cleanly into the application slot, verifies cleanly, and never
//! runs — the board on the bench during development turned out to be exactly that. So they are
//! refused **by name here** and again **by reset vector** in [`ImageBundle::validate`]; one check
//! is a policy and two are a guarantee.
//!
//! # Paths are resolved from the package or source tree, not the working directory
//!
//! A production package carries a `firmware/` tree beside the executable and that portable tree
//! wins when present; `PORTAL_FIRMWARE_DIR` overrides both, for a bench flashing a one-off image.
//! Development builds fall back to the repository root baked in through `CARGO_MANIFEST_DIR`, so
//! moving `target/` stays harmless and only moving the *source* breaks it.
//!
//! The packaged tree deliberately *mirrors* the repository's shape
//! (`PortalFW/.pio/build/<env>/firmware.bin`), so `discover_in` is one implementation serving both
//! and there is no second discovery path to keep in agreement.
//!
//! A missing `.pio` is a first-class state with a build hint attached, not an error — the firmware
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
    /// Which PCB an application bank was built for -- `optical` or `mechanical`. Data from
    /// [`APPLICATION_ENVS`], never parsed back out of the label. `None` for a bootloader.
    pub variant: Option<String>,
    /// The hardware revision that variant targets, e.g. `PCB v6`. `None` for a bootloader.
    pub hardware: Option<String>,
    /// The version banner scraped out of the file's own bytes -- `Portal v2026-08-25_17.34
    /// 8799276+`, `Bootloader v5` -- so a picker can name the build rather than the file.
    /// `None` when the file has none or could not be read; the artefact is listed either way.
    pub banner: Option<String>,
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
    /// A default application pick for the common case: the optical (v6, production-default)
    /// build if it exists, otherwise whichever application variant is available. Never a
    /// substitute for the operator's own board-type choice — see the module docs — the picker
    /// offers every discovered application artefact regardless of what this returns.
    pub fn application(&self) -> Option<&Artefact> {
        let apps = || {
            self.found
                .iter()
                .filter(|a| a.region == RegionName::Application)
        };
        apps()
            .find(|a| a.id == "portalfw:application_bank_optical")
            .or_else(|| apps().next())
    }

    /// Every discovered application artefact, across both PCB variants — what the board-type
    /// picker actually shows.
    pub fn applications(&self) -> impl Iterator<Item = &Artefact> {
        self.found
            .iter()
            .filter(|a| a.region == RegionName::Application)
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
    if let Some(root) = std::env::var_os("PORTAL_FIRMWARE_ROOT") {
        return PathBuf::from(root);
    }
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest.join("..").join("..").join("..");
    root.canonicalize().unwrap_or(root)
}

/// The environment variable that overrides where firmware is looked for.
///
/// First in [`artefact_root`]'s order, because it is the only arm an operator can reach without
/// rebuilding or repackaging: a bench that must flash a one-off image points at a directory and
/// gets the same discovery, the same validation and the same log line as every other run.
pub const FIRMWARE_DIR_ENV: &str = "PORTAL_FIRMWARE_DIR";

/// Where a packaged copy of this application keeps the files it was shipped with, or `None` when
/// it is running out of a source tree.
///
/// # Why this exists at all
///
/// Everything else in this module resolves against [`repo_root`], which is `CARGO_MANIFEST_DIR`
/// baked in at compile time. That is exactly right for a developer -- moving `target/` is
/// harmless -- and exactly wrong for a distributable, where the binary is the only thing that
/// travels and the repository it was built from does not exist on the far machine. Without this
/// the packaged bench comes up, enumerates its probe, and offers nothing to flash.
///
/// Resolved from the executable, never the process working directory, so double-click, a
/// shortcut and a command line all agree.
pub fn resources_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    resource_roots(exe.parent()?)
        .into_iter()
        .find(|root| firmware_tree(&root.join("firmware")).is_some())
}

/// The two package layouts, in the order they are tried.
///
/// - `<exe dir>` -- the unzipped-directory layout, which is what Windows ships.
/// - `<exe dir>/../Resources` -- macOS, where the executable lives in `Contents/MacOS`. Gated on
///   the parent actually being named `MacOS`, so a `Resources` directory that happens to sit
///   beside a development build is not mistaken for a bundle.
///
/// Neither is gated on the host OS. A `.app` is a directory and a zip is a directory, and a
/// developer who unpacks one on the other platform to look inside should get the same answer the
/// operator gets rather than a silently different one.
fn resource_roots(exe_dir: &Path) -> Vec<PathBuf> {
    let mut roots = vec![exe_dir.to_path_buf()];
    if exe_dir.file_name().is_some_and(|name| name == "MacOS")
        && let Some(contents) = exe_dir.parent()
    {
        roots.push(contents.join("Resources"));
    }
    roots
}

/// A directory that actually holds a repository-shaped firmware tree, canonicalised.
///
/// Checked by the `.pio/build` directories rather than by the `firmware/` directory merely
/// existing: an empty or half-copied payload should fall through to the next candidate rather
/// than win and then find nothing.
fn firmware_tree(root: &Path) -> Option<PathBuf> {
    let has_application = root.join("PortalFW/.pio/build").is_dir();
    let has_bootloader = root.join("PortalBootloader/.pio/build").is_dir();
    (has_application || has_bootloader)
        .then(|| root.canonicalize().unwrap_or_else(|_| root.to_path_buf()))
}

/// A self-contained release keeps the repository-shaped firmware payload under
/// `<exe directory>/firmware`, or `Contents/Resources/firmware` inside a macOS bundle.
fn packaged_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    packaged_root_from(exe.parent()?)
}

fn packaged_root_from(exe_dir: &Path) -> Option<PathBuf> {
    resource_roots(exe_dir)
        .into_iter()
        .find_map(|root| firmware_tree(&root.join("firmware")))
}

/// The directory [`discover`] resolves every artefact path against.
///
/// Three arms in falling order of specificity, and the order is the whole design: an explicit
/// request beats what was shipped, and what was shipped beats what was compiled in. The last arm
/// is the historical behaviour, unchanged, so a developer's tree behaves exactly as it did.
///
/// The chosen root travels into [`Discovery::root`], which the page and the session log both
/// print -- so "it found nothing" is always answerable without a debugger.
pub fn artefact_root() -> PathBuf {
    std::env::var_os(FIRMWARE_DIR_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(packaged_root)
        .unwrap_or_else(repo_root)
}

/// What is available to flash, right now.
pub fn discover() -> Discovery {
    discover_in(&artefact_root())
}

/// The same, rooted anywhere — which is what makes it testable without a built firmware tree.
pub fn discover_in(root: &Path) -> Discovery {
    let mut found = Vec::new();
    let mut missing = Vec::new();

    // ---- the application: one entry per PCB revision, see the module docs
    for (env, label, variant, hardware) in APPLICATION_ENVS {
        let app_dir = root.join("PortalFW/.pio/build").join(env);
        let app_bin = app_dir.join("firmware.bin");
        match stat(&app_bin) {
            Some((bytes, modified)) => found.push(Artefact {
                id: format!("portalfw:{env}"),
                label: (*label).into(),
                region: RegionName::Application,
                origin: Origin::Built,
                banner: banner_of(&app_bin),
                path: app_bin,
                bytes,
                modified,
                elf: exists(app_dir.join("firmware.elf")),
                variant: Some((*variant).into()),
                hardware: Some((*hardware).into()),
            }),
            None => missing.push(Missing {
                label: (*label).into(),
                path: app_bin,
                hint: format!("not built yet — run `pio run -e {env}` in PortalFW"),
            }),
        }
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
            banner: banner_of(&boot_bin),
            path: boot_bin,
            bytes,
            modified,
            elf: exists(boot_dir.join("firmware.elf")),
            variant: None,
            hardware: None,
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
            banner: banner_of(&reference),
            path: reference,
            bytes,
            modified,
            elf: None,
            variant: None,
            hardware: None,
        });
    }

    Discovery {
        found,
        missing,
        root: Some(root.to_path_buf()),
    }
}

/// The two PCB revisions in production: the env each one builds as, its label, and the variant
/// and hardware revision a picker can show on their own. Kept as data so discovery and the
/// module docs stay in sync with what `PortalFW/platformio.ini` actually defines.
const APPLICATION_ENVS: &[(&str, &str, &str, &str)] = &[
    (
        "application_bank_optical",
        "PortalFW application (optical, PCB v6)",
        "optical",
        "PCB v6",
    ),
    (
        "application_bank_mechanical",
        "PortalFW application (mechanical, PCB v4)",
        "mechanical",
        "PCB v4",
    ),
];

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
    APPLICATION_ENVS.iter().any(|(name, ..)| *name == env)
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

/// The version banner in a file's bytes, if it has one. The files are tens of kilobytes and
/// discovery runs on a rescan, so reading them whole is cheaper than being clever.
fn banner_of(path: &Path) -> Option<String> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| crate::device::first_banner(&bytes))
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
    fn a_firmware_tree_beside_the_executable_is_a_portable_root() {
        let exe_dir = scratch("portable");
        write(
            &exe_dir,
            "firmware/PortalFW/.pio/build/application_bank_optical/firmware.bin",
            60_000,
        );

        let root = packaged_root_from(&exe_dir).expect("portable firmware root");
        assert_eq!(
            root,
            exe_dir.join("firmware").canonicalize().unwrap(),
            "the package root is independent of the build machine's source path"
        );
        assert!(discover_in(&root).application().is_some());
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
                .any(|m| m.hint.contains("pio run -e application_bank_optical")),
            "a missing application should carry the command that produces it"
        );
    }

    #[test]
    fn a_built_application_is_found_with_its_elf() {
        let root = scratch("app");
        write(
            &root,
            "PortalFW/.pio/build/application_bank_optical/firmware.bin",
            60_000,
        );
        write(
            &root,
            "PortalFW/.pio/build/application_bank_optical/firmware.elf",
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
    fn both_pcb_variants_are_offered_distinctly_and_optical_is_the_default_pick() {
        let root = scratch("variants");
        write(
            &root,
            "PortalFW/.pio/build/application_bank_optical/firmware.bin",
            60_000,
        );
        write(
            &root,
            "PortalFW/.pio/build/application_bank_mechanical/firmware.bin",
            61_000,
        );

        let found = discover_in(&root);
        let apps: Vec<_> = found.applications().collect();
        assert_eq!(
            apps.len(),
            2,
            "both variants should be offered side by side"
        );
        assert!(
            apps.iter()
                .any(|a| a.id == "portalfw:application_bank_optical"),
            "optical should be discoverable by a stable id"
        );
        assert!(
            apps.iter()
                .any(|a| a.id == "portalfw:application_bank_mechanical"),
            "mechanical should be discoverable by a stable id"
        );
        assert_ne!(
            apps[0].label, apps[1].label,
            "the picker distinguishes them by label, so the labels must differ"
        );

        // No hardware strap tells the flasher which board is attached (see the module docs), so
        // the default pick is a starting point for the common case, not a guess at the truth.
        assert_eq!(
            found.application().unwrap().id,
            "portalfw:application_bank_optical",
            "optical is the production default"
        );
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
    fn only_the_two_pcb_variant_envs_may_supply_the_application() {
        assert!(env_supplies_application("application_bank_optical"));
        assert!(env_supplies_application("application_bank_mechanical"));
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
        // Not via `bootloader()`, which deliberately prefers a *built* one -- see
        // `a_built_bootloader_wins_over_the_reference`. This test is about the reference being
        // found at all, and reading it through the preference made it start failing the moment
        // the PlatformIO port produced a build, which is the one outcome that should not break it.
        let boot = found
            .found
            .iter()
            .find(|a| a.region == RegionName::Bootloader && a.origin == Origin::Reference)
            .expect("the committed reference bootloader should be discoverable");
        assert_eq!(boot.bytes, 22_708, "the reference image is 22,708 bytes");
        assert!(boot.fits(), "it must fit the 24 kB bootloader bank");
    }

    #[test]
    fn a_built_bootloader_in_this_repository_supersedes_the_reference() {
        let root = repo_root();
        let built = root.join("PortalBootloader/.pio/build/bootloader/firmware.bin");
        if !built.is_file() {
            eprintln!("skipping: PortalBootloader has not been built here");
            return;
        }
        let found = discover_in(&root);
        assert_eq!(
            found.bootloader().map(|a| a.origin),
            Some(Origin::Built),
            "a built bootloader should win over the committed reference"
        );
    }

    // ------------------------------------------------------------- packaged layouts

    /// The macOS layout: `Contents/MacOS/<exe>` and `Contents/Resources/firmware`.
    ///
    /// The companion to `a_firmware_tree_beside_the_executable_is_a_portable_root` above: a
    /// bundle cannot put the payload beside the executable, because `Contents/MacOS` is for
    /// executables and Finder, `codesign` and every convention expect data in `Resources`.
    #[test]
    fn a_macos_bundle_resolves_contents_resources() {
        let root = scratch("bundle");
        let macos = root.join("PortalTestBench.app/Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        let resources = root.join("PortalTestBench.app/Contents/Resources");
        write(
            &resources,
            "firmware/PortalFW/.pio/build/application_bank_optical/firmware.bin",
            60_000,
        );

        let found = packaged_root_from(&macos).expect("bundled firmware root");
        assert_eq!(found, resources.join("firmware").canonicalize().unwrap());
        assert!(discover_in(&found).application().is_some());
    }

    /// A `Resources` directory one level up is only a bundle when the executable is in `MacOS`.
    /// Otherwise a development tree that happens to have one beside it would be read as packaged,
    /// and the artefacts an operator was shown would come from somewhere nobody chose.
    #[test]
    fn a_resources_directory_is_not_a_bundle_unless_the_exe_is_in_macos() {
        let root = scratch("not-a-bundle");
        write(
            &root.join("Resources"),
            "firmware/PortalFW/.pio/build/application_bank_optical/firmware.bin",
            60_000,
        );
        std::fs::create_dir_all(root.join("debug")).unwrap();
        assert_eq!(packaged_root_from(&root.join("debug")), None);
    }

    /// An empty or half-copied payload falls through rather than winning and then finding
    /// nothing. This is why the check is for the `.pio/build` directories and not for `firmware/`
    /// merely existing.
    #[test]
    fn an_empty_firmware_directory_is_not_a_package() {
        let root = scratch("empty-payload");
        std::fs::create_dir_all(root.join("firmware")).unwrap();
        assert_eq!(packaged_root_from(&root), None);
    }

    #[test]
    fn a_source_tree_is_not_a_package() {
        let root = scratch("source");
        std::fs::create_dir_all(root.join("target/release")).unwrap();
        assert_eq!(packaged_root_from(&root.join("target/release")), None);
    }

    /// The one arm an operator can reach without rebuilding or repackaging.
    ///
    /// Asserted through the env var rather than a pure helper, and serialised against the other
    /// tests that read it, because `set_var` is `unsafe` under edition 2024 precisely because it
    /// races -- so the lock is the honest way to test the real function rather than a stand-in.
    #[test]
    fn an_explicit_firmware_directory_beats_everything_else() {
        use std::sync::Mutex;
        static ENV: Mutex<()> = Mutex::new(());
        let _guard = ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

        let root = scratch("explicit");
        write(
            &root,
            "PortalFW/.pio/build/application_bank_optical/firmware.bin",
            60_000,
        );

        // SAFETY: serialised by ENV against every other test that touches this variable, and
        // restored before the guard is dropped.
        unsafe { std::env::set_var(FIRMWARE_DIR_ENV, &root) };
        let chosen = artefact_root();
        unsafe { std::env::remove_var(FIRMWARE_DIR_ENV) };

        assert_eq!(chosen, root);
        assert!(discover_in(&chosen).application().is_some());
    }

    #[test]
    fn a_developer_tree_still_resolves_against_the_repository() {
        // The historical behaviour, unchanged. Everything above is additive.
        use std::sync::Mutex;
        static ENV: Mutex<()> = Mutex::new(());
        let _guard = ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        unsafe { std::env::remove_var(FIRMWARE_DIR_ENV) };

        assert_eq!(artefact_root(), repo_root());
    }

    #[test]
    fn a_directory_is_not_mistaken_for_a_binary() {
        let root = scratch("dir");
        std::fs::create_dir_all(
            root.join("PortalFW/.pio/build/application_bank_optical/firmware.bin"),
        )
        .unwrap();
        assert!(discover_in(&root).application().is_none());
    }
}

// ---------------------------------------------------------------- loading

use crate::image::{ImageBundle, OptionBytePolicy, Provenance, Region, RunCheckSpec};
use crate::symbols;

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
    /// The run-check spec for this selection, resolved from the application's ELF where it can be.
    ///
    /// Deliberately not an error when it cannot. Three ordinary situations produce no liveness
    /// address — a bootloader-only flash, a build from before `g_liveness_counter` existed, and an
    /// artefact shipped as a `.bin` with no `.elf` beside it — and none of them is a reason to
    /// refuse to programme a good image. `ImageBundle::warnings` reports it, the operator sees it,
    /// and `Rig::run_check` refuses on its own if anyone tries to run one anyway.
    ///
    /// `vtor` is always set: it is the application's load address, a fact about the layout rather
    /// than about the build, and it is what catches the specific failure of a board that came out
    /// of reset into the system ROM instead of into our firmware.
    fn run_check_for(&self, selection: &Selection) -> RunCheckSpec {
        let spec = RunCheckSpec::default();
        let Some(elf) = selection
            .application
            .as_deref()
            .and_then(|id| self.by_id(id))
            .and_then(|artefact| artefact.elf.as_deref())
        else {
            return spec;
        };
        match symbols::liveness_address(elf) {
            Ok(address) => RunCheckSpec {
                liveness_address: address,
                liveness_symbol: symbols::LIVENESS_SYMBOL.to_owned(),
                ..spec
            },
            Err(_) => spec,
        }
    }

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
            run_check: self.run_check_for(selection),
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
        let app = dir.join("PortalFW/.pio/build/application_bank_optical");
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
        let app = dir.join("PortalFW/.pio/build/application_bank_optical/firmware.bin");
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
    fn an_application_with_no_elf_beside_it_warns_rather_than_refuses() {
        // `tree` writes a firmware.bin and no firmware.elf, which is what a `.bin` handed over on
        // its own looks like. Flashable, but not automatically verifiable -- and that has to be
        // visible rather than silently absent.
        let found = discover_in(&tree("warn"));
        let bundle = found
            .load(&Selection {
                bootloader: None,
                application: found.application().map(|a| a.id.clone()),
            })
            .unwrap();
        assert_eq!(bundle.validate(), vec![]);
        assert_eq!(bundle.run_check.liveness_address, 0);
        assert!(
            bundle
                .warnings()
                .contains(&crate::image::BundleFault::NoLivenessAddress)
        );
    }

    /// The same tree, plus a firmware.elf that resolves.
    fn tree_with_elf(name: &str, symbols: &[(&str, u64, u64)]) -> PathBuf {
        let dir = tree(name);
        let app = dir.join("PortalFW/.pio/build/application_bank_optical");
        std::fs::write(app.join("firmware.elf"), crate::symbols::elf_with(symbols)).unwrap();
        dir
    }

    #[test]
    fn a_liveness_address_comes_from_the_elf_beside_the_binary() {
        let root = tree_with_elf("liveness", &[("g_liveness_counter", 0x2000_0180, 4)]);
        let found = discover_in(&root);
        let bundle = found
            .load(&Selection {
                bootloader: None,
                application: found.application().map(|a| a.id.clone()),
            })
            .unwrap();

        assert_eq!(bundle.run_check.liveness_address, 0x2000_0180);
        assert_eq!(bundle.run_check.liveness_symbol, "g_liveness_counter");
        // VTOR is a fact about the layout, not about the build, so it is set either way.
        assert_eq!(bundle.run_check.vtor, addr::APP_BASE);
        assert_eq!(bundle.warnings(), vec![]);
    }

    #[test]
    fn firmware_predating_the_counter_still_flashes() {
        // Every build before the counter was added. Refusing to programme a perfectly good image
        // because a *later* verification step cannot run would be the tail wagging the dog.
        let root = tree_with_elf("old", &[("setup", 0x0800_6200, 4)]);
        let found = discover_in(&root);
        let bundle = found
            .load(&Selection {
                bootloader: None,
                application: found.application().map(|a| a.id.clone()),
            })
            .expect("an older firmware is still flashable");

        assert_eq!(bundle.validate(), vec![]);
        assert_eq!(bundle.run_check.liveness_address, 0);
    }

    #[test]
    fn a_bootloader_only_flash_has_nothing_to_run_check() {
        // There is no application to run, so an absent liveness address is the correct answer
        // rather than a degraded one -- and the reference bootloader has no ELF anyway.
        let root = tree_with_elf("bootonly", &[("g_liveness_counter", 0x2000_0180, 4)]);
        let found = discover_in(&root);
        let bundle = found
            .load(&Selection {
                bootloader: found.bootloader().map(|a| a.id.clone()),
                application: None,
            })
            .unwrap();
        assert_eq!(bundle.run_check.liveness_address, 0);
    }
}
