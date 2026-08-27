//! Turning an ELF into the flat image the firmware routes take.
//!
//! Everything downstream of `firmware_upload` -- `FwSession::new`, `bootloader_update::validate`,
//! `RepeaterImage::new` -- takes a flat `&[u8]`, which is what `objcopy -O binary` produces and
//! what `.pio/build/<env>/firmware.bin` is. The `.elf` beside it is the same image with a symbol
//! table and debug info wrapped around it, and it is at least as likely to be the file somebody
//! sends: it is what a debugger session leaves behind and what CI tends to publish first.
//!
//! # Hand-rolled rather than pulling in `object`
//!
//! This is a fixed-layout read of ELF32's program header table and nothing else -- no sections, no
//! symbols, no relocations, no 64-bit, no big-endian. `PortalFlasher`'s `portal-swd` uses the
//! `object` crate for the same job because it already depends on it to resolve a run-check symbol
//! out of the same files; nothing here needs a symbol table, and `router-proto` next door has
//! exactly two dependencies on purpose. The two implementations are checked against the same
//! committed reference pair, so "the same output" is a test rather than a claim.
//!
//! # Physical addresses, not virtual ones
//!
//! `.data` decides this. Its contents live in flash and are copied to RAM at startup, so its
//! program header carries `p_vaddr` in SRAM and `p_paddr` in flash. Laying segments out by
//! `p_vaddr` would place the initialised-data block at `0x2000....` and describe an image spanning
//! half a gigabyte. `objcopy -O binary` uses `p_paddr`; so does this.
//!
//! Gaps are filled with `0xFF` because erased flash reads as `0xFF`, and the space between two
//! segments is flash no segment claimed. Zero-filling would program bytes the linker never asked
//! for, and the verify pass would then be checking our invention.

use router_proto::layout;

/// The magic every ELF starts with.
const ELFMAG: [u8; 4] = [0x7F, b'E', b'L', b'F'];
const ELFCLASS32: u8 = 1;
const ELFDATA2LSB: u8 = 1;
const PT_LOAD: u32 = 1;

/// Offsets inside the 52-byte ELF32 header.
const E_PHOFF: usize = 0x1C;
const E_PHENTSIZE: usize = 0x2A;
const E_PHNUM: usize = 0x2C;
const EHSIZE: usize = 52;

/// Offsets inside one 32-byte ELF32 program header.
const P_TYPE: usize = 0x00;
const P_OFFSET: usize = 0x04;
const P_PADDR: usize = 0x0C;
const P_FILESZ: usize = 0x10;
const PHENTSIZE: usize = 32;

#[derive(Debug, PartialEq, Eq)]
pub enum ElfError {
    NotAnElf,
    /// An ELF, but not 32-bit little-endian, so not a build for this part.
    NotThisTarget,
    /// Structurally broken: a header that points outside the file.
    Malformed,
    /// Parsed, and nothing in it is loadable -- an object file, or debug info alone.
    NoLoadableSegments,
    /// Loadable, but not describing one image in this part's flash.
    Span {
        base: u32,
        end: u64,
    },
}

impl std::fmt::Display for ElfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ElfError::NotAnElf => write!(f, "not an ELF"),
            ElfError::NotThisTarget => write!(
                f,
                "not a 32-bit little-endian ELF, so not a build for the STM32G070"
            ),
            ElfError::Malformed => write!(f, "the ELF's program headers point outside the file"),
            ElfError::NoLoadableSegments => write!(
                f,
                "the ELF has no loadable segments -- an object file or debug info, not a linked image"
            ),
            ElfError::Span { base, end } => write!(
                f,
                "the loadable segments span {base:#010X}..{end:#010X}, which is not one image in \
                 this part's flash"
            ),
        }
    }
}

/// Whether these bytes begin with the ELF magic. Routing, not validation.
pub fn is_elf(bytes: &[u8]) -> bool {
    bytes.starts_with(&ELFMAG)
}

fn word(bytes: &[u8], at: usize) -> Option<u32> {
    let slice = bytes.get(at..at + 4)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn half(bytes: &[u8], at: usize) -> Option<u16> {
    let slice = bytes.get(at..at + 2)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

/// Lay an ELF's loadable segments out into one flat image, as `objcopy -O binary` would.
///
/// Returns the image and the lowest physical address any loadable segment claims.
pub fn flatten(bytes: &[u8]) -> Result<(u32, Vec<u8>), ElfError> {
    if !is_elf(bytes) {
        return Err(ElfError::NotAnElf);
    }
    if bytes.len() < EHSIZE {
        return Err(ElfError::Malformed);
    }
    if bytes[4] != ELFCLASS32 || bytes[5] != ELFDATA2LSB {
        return Err(ElfError::NotThisTarget);
    }
    let phoff = word(bytes, E_PHOFF).ok_or(ElfError::Malformed)? as usize;
    let phentsize = half(bytes, E_PHENTSIZE).ok_or(ElfError::Malformed)? as usize;
    let phnum = half(bytes, E_PHNUM).ok_or(ElfError::Malformed)? as usize;
    if phnum == 0 || phoff == 0 {
        return Err(ElfError::NoLoadableSegments);
    }
    // A header table shorter than the fields read below would make every read a silent zero.
    if phentsize < PHENTSIZE {
        return Err(ElfError::Malformed);
    }

    // `p_filesz == 0` is skipped alongside non-PT_LOAD: `.bss` is loadable and has no contents, so
    // letting it set the base or the end would stretch the image over RAM nobody asked to program.
    let mut segments = Vec::new();
    for index in 0..phnum {
        let at = phoff + index * phentsize;
        let header = bytes.get(at..at + PHENTSIZE).ok_or(ElfError::Malformed)?;
        if word(header, P_TYPE).ok_or(ElfError::Malformed)? != PT_LOAD {
            continue;
        }
        let filesz = word(header, P_FILESZ).ok_or(ElfError::Malformed)? as usize;
        if filesz == 0 {
            continue;
        }
        let offset = word(header, P_OFFSET).ok_or(ElfError::Malformed)? as usize;
        let paddr = word(header, P_PADDR).ok_or(ElfError::Malformed)?;
        let data = bytes
            .get(offset..offset + filesz)
            .ok_or(ElfError::Malformed)?;
        segments.push((paddr, data));
    }

    let base = segments
        .iter()
        .map(|(paddr, _)| *paddr)
        .min()
        .ok_or(ElfError::NoLoadableSegments)?;
    let end = segments
        .iter()
        .map(|(paddr, data)| u64::from(*paddr) + data.len() as u64)
        .max()
        .ok_or(ElfError::NoLoadableSegments)?;

    // Checked before anything is allocated: an ELF laid out by virtual address, or one for another
    // part entirely, would otherwise ask for a buffer measured in hundreds of megabytes.
    let span = end - u64::from(base);
    if span > u64::from(layout::FLASH_END - layout::FLASH_BASE)
        || !(layout::FLASH_BASE..layout::FLASH_END).contains(&base)
    {
        return Err(ElfError::Span { base, end });
    }

    let mut image = vec![0xFF; span as usize];
    for (paddr, data) in segments {
        let at = (paddr - base) as usize;
        image[at..at + data.len()].copy_from_slice(data);
    }
    Ok((base, image))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// The committed reference pair: an ELF and the `.bin` `objcopy` made from it.
    ///
    /// The same fixture `PortalFlasher`'s `portal_swd::elf` uses, which is the point: two
    /// independent implementations checked against one artefact neither of them produced.
    #[test]
    fn flattening_the_reference_elf_reproduces_the_reference_bin() {
        let dir =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../PortalBootloader/reference");
        let Ok(elf) = std::fs::read(dir.join("BootloaderRS485-2023-08-26.elf")) else {
            eprintln!("skipped: PortalBootloader/reference is not present");
            return;
        };
        let bin = std::fs::read(dir.join("BootloaderRS485-2023-08-26.bin")).unwrap();
        let (base, image) = flatten(&elf).expect("the reference ELF flattens");
        assert_eq!(base, layout::FLASH_BASE);
        assert_eq!(image.len(), bin.len());
        assert!(
            image == bin,
            "the flattened image differs from the committed .bin"
        );
    }

    #[test]
    fn a_bin_is_not_an_elf() {
        assert!(!is_elf(&[0x00, 0x90, 0x00, 0x20]));
        assert_eq!(flatten(&[0x00, 0x90, 0x00, 0x20]), Err(ElfError::NotAnElf));
    }

    #[test]
    fn a_truncated_elf_is_malformed_rather_than_a_panic() {
        let mut bytes = vec![0u8; 20];
        bytes[..4].copy_from_slice(&ELFMAG);
        bytes[4] = ELFCLASS32;
        bytes[5] = ELFDATA2LSB;
        assert_eq!(flatten(&bytes), Err(ElfError::Malformed));
    }

    #[test]
    fn a_64_bit_elf_is_refused_by_name() {
        let mut bytes = vec![0u8; EHSIZE];
        bytes[..4].copy_from_slice(&ELFMAG);
        bytes[4] = 2; // ELFCLASS64
        bytes[5] = ELFDATA2LSB;
        assert_eq!(flatten(&bytes), Err(ElfError::NotThisTarget));
    }
}
