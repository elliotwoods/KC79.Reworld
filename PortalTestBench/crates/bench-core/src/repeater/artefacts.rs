//! Which repeater images this machine has, and what each one is.
//!
//! Deliberately separate from `portal_swd::artefacts`, which walks the same kind of tree for
//! the same kind of reason. That one identifies an STM32 image by a `KC79APP1` descriptor at
//! `base + 0xC0` and a `"Portal v"` banner scraped out of the bytes; asking it about an
//! ESP32 image would either refuse it or -- worse -- take it for an application and hand it
//! to an ST-Link. Two chips, two classifiers, no shared vocabulary between them.
//!
//! Everything here is read from the bytes. There is no manifest, for the same reason there
//! is none on the Portal side: a file that says what an image is can disagree with the image.

use std::path::{Path, PathBuf};

use router_link::repeater_ota::{sha256, RepeaterOtaParams, APP_SLOT_BYTES};
use serde::Serialize;

/// Where PlatformIO leaves the repeater build, relative to a repository-shaped root.
pub const BUILD_DIR: &str = "RS485Repeater/.pio/build/repeater";

/// How to rebuild, when the tree has no images in it.
pub const BUILD_HINT: &str = "not built yet -- run `pio run -d RS485Repeater -e repeater`";

/// First byte of any ESP image.
const ESP_MAGIC: u8 = 0xE9;

/// `chip_id` in the extended image header, little-endian at offset 12.
const CHIP_ID_ESP32C3: u16 = 5;

/// Where the partition table sits in a merged image, and the two bytes that say it is one.
const PARTITION_TABLE_OFFSET: usize = 0x8000;
const PARTITION_TABLE_MAGIC: [u8; 2] = [0xAA, 0x50];

/// Where `app0` starts. A merged image is the bootloader, the table, `otadata` and the
/// application in one blob from zero; an application image is what lives here on its own.
pub const APP_OFFSET: usize = 0x10000;

/// The NVS partition, which a merged write at offset 0 covers with `0xFF`.
pub const NVS_RANGE: (usize, usize) = (0x9000, 0xE000);

/// A whole 4 MB part, which is the ceiling on a merged image.
const FLASH_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RepeaterImageKind {
    /// Bootloader + partition table + otadata + application, written at offset 0 over USB.
    Factory,
    /// The application alone, sent into the spare OTA slot over RS485.
    Application,
}

impl RepeaterImageKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RepeaterImageKind::Factory => "factory",
            RepeaterImageKind::Application => "application",
        }
    }
}

/// What an ESP image is, from its own bytes.
///
/// The three facts that decide it, and where each is checked:
///
/// | | |
/// |---|---|
/// | `bytes[0] == 0xE9` | it is an ESP image at all |
/// | `u16le` at `0x0C` is `5` | it is for an ESP32-C3, not an S3 dev board on the same desk |
/// | a partition-table magic at `0x8000` and a second image header at `0x10000` | it is merged |
pub fn classify(bytes: &[u8]) -> Result<RepeaterImageKind, String> {
    if bytes.len() < 24 {
        return Err(format!("{} bytes is too short to be an image", bytes.len()));
    }
    if bytes[0] != ESP_MAGIC {
        return Err(format!(
            "does not start with the ESP image magic 0xE9 (found 0x{:02X}) -- this is not \
             repeater firmware",
            bytes[0]
        ));
    }
    let chip = u16::from_le_bytes([bytes[12], bytes[13]]);
    if chip != CHIP_ID_ESP32C3 {
        return Err(format!(
            "built for {}, not the ESP32-C3 a repeater carries",
            chip_name(chip)
        ));
    }
    let merged = bytes.len() > APP_OFFSET
        && bytes[PARTITION_TABLE_OFFSET..PARTITION_TABLE_OFFSET + 2] == PARTITION_TABLE_MAGIC
        && bytes[APP_OFFSET] == ESP_MAGIC;
    if merged {
        if bytes.len() > FLASH_BYTES {
            return Err(format!(
                "{} bytes will not fit a 4 MB part",
                bytes.len()
            ));
        }
        return Ok(RepeaterImageKind::Factory);
    }
    if bytes.len() > APP_SLOT_BYTES {
        return Err(format!(
            "{} bytes is more than an OTA slot holds ({APP_SLOT_BYTES})",
            bytes.len()
        ));
    }
    Ok(RepeaterImageKind::Application)
}

/// Espressif's `chip_id`s, for the ones that turn up on a bench with the right cable.
fn chip_name(id: u16) -> String {
    match id {
        0 => "an ESP32".into(),
        2 => "an ESP32-S2".into(),
        5 => "an ESP32-C3".into(),
        9 => "an ESP32-S3".into(),
        12 => "an ESP32-C2".into(),
        13 => "an ESP32-C6".into(),
        16 => "an ESP32-H2".into(),
        other => format!("chip id {other}"),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RepeaterArtefact {
    /// Stable across builds, because it names a role rather than a build: the picker's
    /// selection has to survive a rebuild.
    pub id: String,
    pub label: String,
    pub kind: RepeaterImageKind,
    pub path: PathBuf,
    pub bytes: u64,
    pub modified: u64,
    pub sha256: String,
    /// Application images only: what an in-band transfer costs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunks: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_seconds: Option<f32>,
    pub fits: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepeaterMissing {
    pub label: String,
    pub path: PathBuf,
    pub hint: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepeaterDiscovery {
    pub root: PathBuf,
    pub found: Vec<RepeaterArtefact>,
    pub missing: Vec<RepeaterMissing>,
    /// Set when both images are present and the merged one does *not* carry the same
    /// application. Two routes installing two different builds is the one way this pair can
    /// be wrong, and it is invisible in either file on its own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mismatch: Option<String>,
}

impl RepeaterDiscovery {
    pub fn by_id(&self, id: &str) -> Option<&RepeaterArtefact> {
        self.found.iter().find(|item| item.id == id)
    }

    pub fn of_kind(&self, kind: RepeaterImageKind) -> Option<&RepeaterArtefact> {
        self.found.iter().find(|item| item.kind == kind)
    }
}

/// The two images a repository-shaped tree holds, wherever that tree is.
pub fn discover_in(root: &Path) -> RepeaterDiscovery {
    let dir = root.join(BUILD_DIR);
    let mut found = Vec::new();
    let mut missing = Vec::new();

    for (file, id, label) in [
        (
            "firmware.factory.bin",
            "repeater:factory",
            "RS485 repeater -- merged factory image",
        ),
        (
            "firmware.bin",
            "repeater:application",
            "RS485 repeater -- application (RS485 OTA)",
        ),
    ] {
        let path = dir.join(file);
        match read_artefact(&path, id, label) {
            Ok(artefact) => found.push(artefact),
            Err(reason) => missing.push(RepeaterMissing {
                label: label.to_string(),
                path,
                hint: reason,
            }),
        }
    }

    let mismatch = check_pair(&dir, &found);
    RepeaterDiscovery {
        root: root.to_path_buf(),
        found,
        missing,
        mismatch,
    }
}

fn read_artefact(path: &Path, id: &str, label: &str) -> Result<RepeaterArtefact, String> {
    let bytes = std::fs::read(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            BUILD_HINT.to_string()
        } else {
            format!("could not be read: {error}")
        }
    })?;
    let kind = classify(&bytes)?;
    let modified = std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| since.as_secs())
        .unwrap_or(0);
    let (chunks, estimated_seconds) = match kind {
        RepeaterImageKind::Application => {
            let params = RepeaterOtaParams::default();
            let chunks = bytes.len().div_ceil(params.chunk_bytes);
            let image = router_link::repeater_ota::RepeaterImage::new(bytes.clone(), params.chunk_bytes)
                .map_err(|error| error.to_string())?;
            (Some(chunks), Some(image.estimated_seconds(&params)))
        }
        RepeaterImageKind::Factory => (None, None),
    };
    Ok(RepeaterArtefact {
        id: id.to_string(),
        label: label.to_string(),
        kind,
        path: path.to_path_buf(),
        bytes: bytes.len() as u64,
        modified,
        sha256: hex(&sha256(&bytes)),
        chunks,
        estimated_seconds,
        fits: true,
    })
}

/// The invariant that keeps the two routes installing one build.
///
/// `firmware.factory.bin[0x10000..]` is `firmware.bin`, byte for byte -- PlatformIO builds the
/// merged image *from* the application. So a tree where they disagree has been half-rebuilt,
/// and provisioning one repeater over USB and its neighbour over RS485 would leave two
/// versions in an installation that reports one.
fn check_pair(dir: &Path, found: &[RepeaterArtefact]) -> Option<String> {
    let factory = found
        .iter()
        .find(|item| item.kind == RepeaterImageKind::Factory)?;
    let application = found
        .iter()
        .find(|item| item.kind == RepeaterImageKind::Application)?;
    let merged = std::fs::read(dir.join("firmware.factory.bin")).ok()?;
    let embedded = merged.get(APP_OFFSET..)?;
    let embedded_sha = hex(&sha256(embedded));
    if embedded_sha == application.sha256 {
        return None;
    }
    Some(format!(
        "{} carries a different application from {}: the tree has been half-rebuilt, so the \
         USB route and the RS485 route would install two different builds. Run \
         `pio run -d RS485Repeater -e repeater` again.",
        factory.path.display(),
        application.path.display()
    ))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 24 bytes that decide what an image is: magic, then the extended header whose
    /// `chip_id` sits at offset 12.
    fn header(chip: u16) -> Vec<u8> {
        let mut bytes = vec![0u8; 24];
        bytes[0] = ESP_MAGIC;
        bytes[12..14].copy_from_slice(&chip.to_le_bytes());
        bytes
    }

    fn application(len: usize) -> Vec<u8> {
        let mut bytes = header(CHIP_ID_ESP32C3);
        bytes.resize(len, 0);
        bytes
    }

    fn factory(app_len: usize) -> Vec<u8> {
        let mut bytes = application(APP_OFFSET);
        bytes[PARTITION_TABLE_OFFSET..PARTITION_TABLE_OFFSET + 2]
            .copy_from_slice(&PARTITION_TABLE_MAGIC);
        bytes.extend(application(app_len));
        bytes
    }

    #[test]
    fn the_two_shapes_are_told_apart_by_their_bytes() {
        assert_eq!(
            classify(&application(1024)).unwrap(),
            RepeaterImageKind::Application
        );
        assert_eq!(
            classify(&factory(1024)).unwrap(),
            RepeaterImageKind::Factory
        );
    }

    #[test]
    fn a_portal_image_is_refused_by_name() {
        // An STM32 image starts with a vector table, not 0xE9. Somebody will drop one here.
        let mut portal = vec![0u8; 256];
        portal[0] = 0x00;
        portal[1] = 0x80;
        let error = classify(&portal).unwrap_err();
        assert!(error.contains("not repeater firmware"), "{error}");
    }

    #[test]
    fn another_espressif_part_is_refused_and_named() {
        let error = classify(&application(1024).into_iter().enumerate().map(|(i, b)| {
            if i == 12 { 9 } else if i == 13 { 0 } else { b }
        }).collect::<Vec<_>>()).unwrap_err();
        assert!(error.contains("ESP32-S3"), "{error}");
    }

    #[test]
    fn an_application_larger_than_a_slot_is_refused_with_both_numbers() {
        let error = classify(&application(APP_SLOT_BYTES + 1)).unwrap_err();
        assert!(error.contains(&APP_SLOT_BYTES.to_string()), "{error}");
        assert!(error.contains(&(APP_SLOT_BYTES + 1).to_string()), "{error}");
    }

    /// The invariant the whole USB route rests on, checked against the real build when there
    /// is one. A future `platformio.ini` that changed the partition scheme would fail here
    /// rather than on somebody's board.
    #[test]
    fn the_real_factory_image_is_the_three_parts_plus_the_app() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let path = root.join(BUILD_DIR).join("firmware.factory.bin");
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("skipped: {} is not built", path.display());
            return;
        };
        assert_eq!(bytes[0], ESP_MAGIC);
        assert_eq!(
            u16::from_le_bytes([bytes[12], bytes[13]]),
            CHIP_ID_ESP32C3
        );
        assert_eq!(
            bytes[PARTITION_TABLE_OFFSET..PARTITION_TABLE_OFFSET + 2],
            PARTITION_TABLE_MAGIC
        );
        assert_eq!(bytes[APP_OFFSET], ESP_MAGIC);
        assert_eq!(classify(&bytes).unwrap(), RepeaterImageKind::Factory);

        // And the fact that makes NVS blanking real rather than theoretical: the merged image
        // is contiguous, so a write at zero covers 0x9000..0xE000 with erased bytes.
        assert!(bytes.len() > NVS_RANGE.1);
        assert!(
            bytes[NVS_RANGE.0..NVS_RANGE.1].iter().all(|b| *b == 0xFF),
            "the merged image no longer blanks NVS -- the pass must stop restoring the index"
        );

        // The application half is the application file, so the two routes install one build.
        let application = std::fs::read(root.join(BUILD_DIR).join("firmware.bin")).unwrap();
        assert_eq!(&bytes[APP_OFFSET..], application.as_slice());
    }

    #[test]
    fn a_tree_with_no_build_names_the_path_and_the_command() {
        let discovery = discover_in(std::path::Path::new("/nonexistent-tree"));
        assert_eq!(discovery.found.len(), 0);
        assert_eq!(discovery.missing.len(), 2);
        assert!(discovery.missing[0].hint.contains("pio run -d RS485Repeater"));
        assert!(discovery.missing[0]
            .path
            .to_string_lossy()
            .contains("firmware.factory.bin"));
    }
}
