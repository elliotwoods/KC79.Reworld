//! Turning an ELF into the flat image a flash write actually takes.
//!
//! # Why this exists at all
//!
//! Every path in this crate flashes a *flat* image: `Region::new` takes a load address and a
//! `Vec<u8>`, and `discover_in` only ever lists `firmware.bin`. The ELF beside it has been read
//! for exactly one thing -- [`crate::symbols::liveness_address`] -- and never as an image.
//!
//! That is fine while firmware arrives through a `.pio` tree, where `objcopy -O binary` has
//! already run. It stops being fine the moment an operator can hand the bench a file: what a
//! colleague sends, what a CI job publishes and what a debugger session leaves behind is as often
//! the `.elf` as the `.bin`, and "wrong extension" is a poor reason to refuse a good image.
//!
//! # Why the *physical* address and not the virtual one
//!
//! `.data` is the case that decides it. Its contents live in flash and are copied to RAM by the
//! startup code, so its program header carries `p_vaddr` in SRAM and `p_paddr` in flash. Laying
//! segments out by `p_vaddr` would put the initialised-data block at `0x2000....`, produce an
//! image spanning the entire 512 MB between the two, and fail the span check below rather than
//! producing something subtly wrong -- which is the good outcome, but only because the check is
//! here. `objcopy -O binary` uses `p_paddr`, and so does this.
//!
//! # Why the gaps are `0xFF`
//!
//! Erased flash reads as `0xFF`, and the padding between segments is flash that no segment
//! claims. Filling with zero would make [`crate::image::ImageBundle`] program bytes the linker
//! never asked for, and a verify-after-write would then be checking our invention rather than the
//! build.

use object::read::elf::{FileHeader, ProgramHeader};
use object::{Endianness, elf};

use crate::addr;

/// A flat image and where it was linked, as `objcopy -O binary` would have produced it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flat {
    /// The lowest physical address any loadable segment claims.
    pub base: u32,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ElfError {
    /// Not an ELF at all, or not one `object` can parse.
    NotAnElf(String),
    /// An ELF, but not a 32-bit little-endian one -- so not a build for this part.
    NotThisTarget,
    /// Parsed, but nothing is loadable. An object file or a debug-info-only artefact.
    NoLoadableSegments,
    /// The segments are loadable but do not describe one flash image.
    Span { base: u32, end: u64 },
}

impl core::fmt::Display for ElfError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ElfError::NotAnElf(detail) => write!(f, "not an ELF this can read: {detail}"),
            ElfError::NotThisTarget => write!(
                f,
                "not a 32-bit little-endian ELF, so not a build for the STM32G070"
            ),
            ElfError::NoLoadableSegments => write!(
                f,
                "the ELF has no loadable segments -- an object file or debug info, not a linked \
                 image"
            ),
            ElfError::Span { base, end } => write!(
                f,
                "the loadable segments span {base:#010X}..{end:#010X}, which is not one image in \
                 this part's {} kB of flash",
                (addr::FLASH_END - addr::FLASH_BASE) / 1024
            ),
        }
    }
}

/// Whether these bytes begin with the ELF magic.
///
/// Used to route a dropped file rather than to validate it: `flatten` is the one that decides
/// whether an ELF is usable, and it says why when it is not.
pub fn is_elf(bytes: &[u8]) -> bool {
    bytes.starts_with(&elf::ELFMAG)
}

/// Lay an ELF's loadable segments out into one flat image.
///
/// Equivalent to `arm-none-eabi-objcopy -O binary`, and checked against it: the test at the foot
/// of this file flattens the committed `BootloaderRS485-2023-08-26.elf` and compares it to the
/// `.bin` beside it, which was produced by that command.
pub fn flatten(bytes: &[u8]) -> Result<Flat, ElfError> {
    let header = elf::FileHeader32::<Endianness>::parse(bytes)
        .map_err(|err| ElfError::NotAnElf(err.to_string()))?;
    let endian = header.endian().map_err(|_| ElfError::NotThisTarget)?;
    if endian != Endianness::Little {
        return Err(ElfError::NotThisTarget);
    }
    let headers = header
        .program_headers(endian, bytes)
        .map_err(|err| ElfError::NotAnElf(err.to_string()))?;

    // `p_filesz == 0` is skipped as well as non-PT_LOAD: `.bss` is a loadable segment with no
    // contents, and letting it set the base or the end would extend the image over RAM the
    // linker never asked to have programmed.
    let loadable = || {
        headers.iter().filter(|ph| {
            ph.p_type(endian) == elf::PT_LOAD && ph.p_filesz(endian) > 0
        })
    };

    let base = loadable()
        .map(|ph| ph.p_paddr(endian))
        .min()
        .ok_or(ElfError::NoLoadableSegments)?;
    let end = loadable()
        .map(|ph| u64::from(ph.p_paddr(endian)) + u64::from(ph.p_filesz(endian)))
        .max()
        .ok_or(ElfError::NoLoadableSegments)?;

    // The span is checked before anything is allocated. An ELF whose segments are laid out by
    // virtual address, or one for a different part entirely, would otherwise ask for a buffer
    // measured in hundreds of megabytes before anything looked at whether it made sense.
    let span = end - u64::from(base);
    let flash = u64::from(addr::FLASH_END - addr::FLASH_BASE);
    if span > flash || !(addr::FLASH_BASE..addr::FLASH_END).contains(&base) {
        return Err(ElfError::Span { base, end });
    }

    let mut image = vec![0xFF; span as usize];
    for ph in loadable() {
        let at = (u64::from(ph.p_paddr(endian)) - u64::from(base)) as usize;
        let data = ph
            .data(endian, bytes)
            .map_err(|()| ElfError::NotAnElf("a segment points outside the file".into()))?;
        image[at..at + data.len()].copy_from_slice(data);
    }

    Ok(Flat { base, bytes: image })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// The committed reference pair: an ELF and the `.bin` `objcopy` made from it.
    fn reference() -> Option<(Vec<u8>, Vec<u8>)> {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../PortalBootloader/reference");
        let elf = std::fs::read(dir.join("BootloaderRS485-2023-08-26.elf")).ok()?;
        let bin = std::fs::read(dir.join("BootloaderRS485-2023-08-26.bin")).ok()?;
        Some((elf, bin))
    }

    #[test]
    fn flattening_the_reference_elf_reproduces_the_reference_bin() {
        let Some((elf, bin)) = reference() else {
            // The reference images are committed, but this crate is also built from packaged
            // trees that do not carry the firmware repository. Skipping is honest; asserting
            // against a file that is not there would only ever fail for the wrong reason.
            eprintln!("skipped: PortalBootloader/reference is not present");
            return;
        };
        let flat = flatten(&elf).expect("the reference ELF flattens");
        assert_eq!(flat.base, addr::FLASH_BASE);
        assert_eq!(
            flat.bytes.len(),
            bin.len(),
            "flattened {} bytes, objcopy produced {}",
            flat.bytes.len(),
            bin.len()
        );
        assert!(
            flat.bytes == bin,
            "the flattened image differs from the committed .bin"
        );
    }

    #[test]
    fn a_bin_is_not_an_elf() {
        assert!(!is_elf(&[0x00, 0x90, 0x00, 0x20]));
        assert!(matches!(
            flatten(&[0x00, 0x90, 0x00, 0x20]),
            Err(ElfError::NotAnElf(_))
        ));
    }

    #[test]
    fn an_elf_with_no_loadable_segments_is_refused() {
        // `symbols`' synthesised ELF has a symbol table and no program headers at all, which is
        // exactly the object-file shape this refuses.
        let elf = crate::symbols::elf_with(&[("g_liveness_counter", 0x2000_0100, 4)]);
        assert!(is_elf(&elf));
        assert_eq!(flatten(&elf), Err(ElfError::NoLoadableSegments));
    }
}
