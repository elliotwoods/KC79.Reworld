//! Reading an application image's own account of where it was linked.
//!
//! # The problem this solves
//!
//! An application built for `0x08004000` and one built for `0x08006000` are both, byte for byte,
//! plausible Cortex-M images: a stack pointer in SRAM, a reset vector with the Thumb bit set
//! somewhere inside the application bank, then code. Nothing distinguishes them until an absolute
//! address is dereferenced, at which point the wrong one hard-faults somewhere unrelated to the
//! mistake. That is a bad failure to have available when the two builds sit side by side in
//! `.pio/build/` with names differing by a suffix.
//!
//! So a v6-era image carries a [`AppDescriptor`] at a fixed offset stating its own base address.
//! The bootloader refuses to *start* an image whose descriptor disagrees with where it is sitting;
//! this module is the host half, which refuses to *send* one.
//!
//! # Images without a descriptor
//!
//! Every application built before this existed has none, and those are exactly the images that
//! need to keep working -- they are what the fielded fleet runs. [`image_base`] falls back to
//! inferring the base from the reset vector, and will only conclude "legacy" — never the new base,
//! which no image predating the descriptor can have been built for. An image that looks like
//! neither is refused rather than guessed at.

use crate::layout;

/// What an image says about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppDescriptor {
    /// The address this image was linked for.
    pub base: u32,
    pub flags: u32,
    /// `PORTAL_VERSION_STRING`, NUL-trimmed.
    pub version: String,
}

/// Why an image's base address could not be established.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImageBaseError {
    #[error("image is {bytes} bytes, too short to hold a vector table")]
    TooShort { bytes: usize },
    #[error(
        "image has no application descriptor and its reset vector 0x{reset_vector:08X} is not \
         inside the legacy application bank, so the bank it was built for cannot be established"
    )]
    Indeterminate { reset_vector: u32 },
    #[error("image declares base 0x{base:08X}, which is not an application base")]
    UnknownBase { base: u32 },
}

/// Where the base address came from, which decides how much it can be trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseSource {
    /// Stated by the image itself.
    Descriptor,
    /// Inferred from the reset vector, for an image built before descriptors existed.
    InferredLegacy,
}

/// Read the descriptor at [`layout::APP_DESCRIPTOR_OFFSET`], if there is one.
pub fn read_descriptor(image: &[u8]) -> Option<AppDescriptor> {
    let at = layout::APP_DESCRIPTOR_OFFSET;
    let bytes = image.get(at..at + layout::APP_DESCRIPTOR_BYTES)?;
    if &bytes[..8] != layout::APP_DESCRIPTOR_MAGIC {
        return None;
    }
    let word = |offset: usize| {
        u32::from_le_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ])
    };
    let version = &bytes[16..16 + layout::APP_VERSION_BYTES];
    let end = version.iter().position(|b| *b == 0).unwrap_or(version.len());
    Some(AppDescriptor {
        base: word(8),
        flags: word(12),
        version: String::from_utf8_lossy(&version[..end]).into_owned(),
    })
}

/// The image's initial stack pointer and reset vector.
pub fn vector_table(image: &[u8]) -> Option<(u32, u32)> {
    let head = image.get(..8)?;
    Some((
        u32::from_le_bytes([head[0], head[1], head[2], head[3]]),
        u32::from_le_bytes([head[4], head[5], head[6], head[7]]),
    ))
}

/// Establish which bank an image was built for.
///
/// The descriptor wins when present. Without one, the only conclusion available is the legacy
/// base, and only when the reset vector actually lands inside the legacy bank -- an image linked
/// at `0x08000000` (the `no_bootloader` builds) reaches neither branch and is refused, which is
/// the same refusal `portal-swd` and `tools/firmware.mjs` apply for the same reason.
pub fn image_base(image: &[u8]) -> Result<(u32, BaseSource), ImageBaseError> {
    if image.len() < 8 {
        return Err(ImageBaseError::TooShort { bytes: image.len() });
    }
    if let Some(descriptor) = read_descriptor(image) {
        if !layout::is_app_base(descriptor.base) {
            return Err(ImageBaseError::UnknownBase {
                base: descriptor.base,
            });
        }
        return Ok((descriptor.base, BaseSource::Descriptor));
    }
    let (_, reset_vector) = vector_table(image).expect("length checked above");
    let entry = reset_vector & !1;
    if reset_vector & 1 == 1 && (layout::APP_BASE_LEGACY..layout::APP_END).contains(&entry) {
        return Ok((layout::APP_BASE_LEGACY, BaseSource::InferredLegacy));
    }
    Err(ImageBaseError::Indeterminate { reset_vector })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an image the way a linker would: vector table, then a descriptor at the fixed offset.
    ///
    /// The reset vector is `| 1`, not `+ 1`: the Thumb bit is a bit, and an entry offset that
    /// happened to be odd would have it cleared by an addition rather than set.
    fn image(base: u32, entry_offset: u32, descriptor: bool) -> Vec<u8> {
        let mut bytes = vec![0u8; 0x400];
        bytes[..4].copy_from_slice(&layout::RAM_END.to_le_bytes());
        bytes[4..8].copy_from_slice(&((base + entry_offset) | 1).to_le_bytes());
        if descriptor {
            let at = layout::APP_DESCRIPTOR_OFFSET;
            bytes[at..at + 8].copy_from_slice(layout::APP_DESCRIPTOR_MAGIC);
            bytes[at + 8..at + 12].copy_from_slice(&base.to_le_bytes());
            bytes[at + 12..at + 16].copy_from_slice(&0u32.to_le_bytes());
            let version = b"Portal v2026-08-25_19.19 ea08436+";
            bytes[at + 16..at + 16 + version.len()].copy_from_slice(version);
        }
        bytes
    }

    #[test]
    fn a_descriptor_states_the_base_and_version() {
        let bytes = image(layout::APP_BASE, 0x241, true);
        let descriptor = read_descriptor(&bytes).unwrap();
        assert_eq!(descriptor.base, layout::APP_BASE);
        assert_eq!(descriptor.flags, 0);
        assert_eq!(descriptor.version, "Portal v2026-08-25_19.19 ea08436+");
        assert_eq!(
            image_base(&bytes).unwrap(),
            (layout::APP_BASE, BaseSource::Descriptor)
        );
    }

    /// The descriptor is authoritative even when the reset vector would suggest otherwise. This is
    /// the case that matters: a new-base image's reset vector is *also* inside the legacy bank
    /// (the banks overlap), so inference alone can never distinguish the two.
    #[test]
    fn the_descriptor_beats_the_reset_vector() {
        // An image linked at the new base whose entry point happens to land above 0x08006000.
        let bytes = image(layout::APP_BASE, 0x4000, true);
        let (_, reset_vector) = vector_table(&bytes).unwrap();
        assert!(
            (layout::APP_BASE_LEGACY..layout::APP_END).contains(&(reset_vector & !1)),
            "this test is only meaningful if inference would get it wrong"
        );
        assert_eq!(
            image_base(&bytes).unwrap(),
            (layout::APP_BASE, BaseSource::Descriptor)
        );
    }

    #[test]
    fn a_legacy_image_is_inferred_from_its_reset_vector() {
        let bytes = image(layout::APP_BASE_LEGACY, 0x241, false);
        assert_eq!(read_descriptor(&bytes), None);
        assert_eq!(
            image_base(&bytes).unwrap(),
            (layout::APP_BASE_LEGACY, BaseSource::InferredLegacy)
        );
    }

    /// The `no_bootloader` builds: linked at 0x08000000, they would program and verify cleanly
    /// into an application slot and never run. Refused here, as everywhere else.
    #[test]
    fn an_image_linked_at_zero_offset_is_refused() {
        let bytes = image(layout::FLASH_BASE, 0x241, false);
        assert!(matches!(
            image_base(&bytes),
            Err(ImageBaseError::Indeterminate { .. })
        ));
    }

    #[test]
    fn a_descriptor_naming_an_impossible_base_is_refused() {
        let mut bytes = image(layout::APP_BASE, 0x241, true);
        let at = layout::APP_DESCRIPTOR_OFFSET + 8;
        bytes[at..at + 4].copy_from_slice(&0x0800_5000u32.to_le_bytes());
        assert_eq!(
            image_base(&bytes),
            Err(ImageBaseError::UnknownBase { base: 0x0800_5000 })
        );
    }

    #[test]
    fn short_and_corrupt_images_are_refused_rather_than_guessed() {
        assert_eq!(image_base(&[]), Err(ImageBaseError::TooShort { bytes: 0 }));
        assert_eq!(
            image_base(&[0u8; 4]),
            Err(ImageBaseError::TooShort { bytes: 4 })
        );

        // Right offset, wrong magic: not a descriptor, so fall through to inference.
        let mut bytes = image(layout::APP_BASE_LEGACY, 0x241, true);
        bytes[layout::APP_DESCRIPTOR_OFFSET] = b'X';
        assert_eq!(read_descriptor(&bytes), None);
        assert_eq!(
            image_base(&bytes).unwrap().1,
            BaseSource::InferredLegacy,
            "a damaged descriptor must not be read as a valid one"
        );

        // An image too short to contain the descriptor offset at all.
        let short = image(layout::APP_BASE_LEGACY, 0x241, false)[..0x40].to_vec();
        assert_eq!(read_descriptor(&short), None);
    }

    /// A reset vector without the Thumb bit is not a Cortex-M entry point, whatever else is true
    /// of the image.
    #[test]
    fn a_non_thumb_reset_vector_is_not_inferred() {
        let mut bytes = image(layout::APP_BASE_LEGACY, 0x241, false);
        bytes[4] &= !1;
        assert!(matches!(
            image_base(&bytes),
            Err(ImageBaseError::Indeterminate { .. })
        ));
    }

    /// The version string fills its field exactly, with no NUL to trim.
    #[test]
    fn a_maximal_version_string_round_trips() {
        let mut bytes = image(layout::APP_BASE, 0x241, true);
        let at = layout::APP_DESCRIPTOR_OFFSET + 16;
        let version = "v".repeat(layout::APP_VERSION_BYTES);
        bytes[at..at + layout::APP_VERSION_BYTES].copy_from_slice(version.as_bytes());
        assert_eq!(read_descriptor(&bytes).unwrap().version, version);
    }
}
