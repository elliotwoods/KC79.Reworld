//! Resolving `g_liveness_counter` out of an application ELF.
//!
//! # Why a symbol at all
//!
//! The run-check has to answer "is this board executing the application" without halting it, and
//! ARMv6-M gives no way to read the program counter without halting the core. Halting a board to
//! find out whether it is running is a contradiction — the halt is indistinguishable from the
//! fault it is looking for.
//!
//! So the firmware counts. `PortalFW/src/main.cpp` declares
//!
//! ```c
//! volatile uint32_t g_liveness_counter = 0;
//! ```
//!
//! and increments it at the top of `loop()`. The rig reads that address twice, a few hundred
//! milliseconds apart, and calls the board good only if the value moved. A board stuck in
//! `HardFault_Handler`, spinning in a watchdog reset loop, or sitting in the system ROM
//! bootloader all present a perfectly healthy debug port and a perfectly still counter.
//!
//! # Why the address is read from the ELF rather than written down
//!
//! Because it moves. It is a linker output, and it changes with any edit that shifts `.data`.
//! A hard-coded address would keep working for weeks and then silently start reading a
//! neighbouring variable, at which point the run-check either passes on a dead board — if the
//! neighbour happens to change — or fails on a live one. Neither failure announces itself.
//!
//! [`RunCheckSpec`](crate::image::RunCheckSpec) therefore carries the address *and* the symbol
//! name it came from, and the bundle that holds it is hashed together with the image bytes, so an
//! address and an image can never be paired by accident.

use std::path::Path;

use object::{Object, ObjectSymbol};

use crate::addr;

/// The symbol the run-check reads. Changing this means changing `PortalFW/src/main.cpp` too.
pub const LIVENESS_SYMBOL: &str = "g_liveness_counter";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SymbolError {
    Unreadable {
        path: String,
        detail: String,
    },
    NotAnElf {
        path: String,
        detail: String,
    },
    Missing {
        symbol: String,
        path: String,
    },
    /// Found, but somewhere a `volatile uint32_t` in `.data` cannot be.
    NotInRam {
        symbol: String,
        address: u64,
    },
    /// Found, but not a whole word, or not word-aligned.
    NotAWord {
        symbol: String,
        address: u64,
        size: u64,
    },
}

impl core::fmt::Display for SymbolError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SymbolError::Unreadable { path, detail } => {
                write!(f, "could not read {path}: {detail}")
            }
            SymbolError::NotAnElf { path, detail } => {
                write!(f, "{path} is not an ELF this can read: {detail}")
            }
            SymbolError::Missing { symbol, path } => write!(
                f,
                "{path} has no `{symbol}` symbol -- the firmware predates the run-check, or was \
                 built with the symbol table stripped"
            ),
            SymbolError::NotInRam { symbol, address } => write!(
                f,
                "`{symbol}` resolves to {address:#010X}, which is not in RAM \
                 ({:#010X}..{:#010X})",
                addr::RAM_BASE,
                addr::RAM_END
            ),
            SymbolError::NotAWord {
                symbol,
                address,
                size,
            } => write!(
                f,
                "`{symbol}` is {size} bytes at {address:#010X}; the run-check reads one aligned \
                 32-bit word"
            ),
        }
    }
}

/// Where `g_liveness_counter` lives, according to this ELF.
pub fn liveness_address(elf: &Path) -> Result<u32, SymbolError> {
    address_of(elf, LIVENESS_SYMBOL)
}

/// The address of a named symbol, checked to be something the run-check can actually read.
///
/// The checks are not ceremony. A symbol table will happily hand back a `.text` address for a
/// same-named function, or a zero-sized linker marker, and the run-check would then poll a
/// constant forever and report every board as dead. Better to refuse here, where the message can
/// say which of those happened, than to produce a spec that fails identically on good hardware.
pub fn address_of(elf: &Path, symbol: &str) -> Result<u32, SymbolError> {
    let path = elf.display().to_string();
    let bytes = std::fs::read(elf).map_err(|err| SymbolError::Unreadable {
        path: path.clone(),
        detail: err.to_string(),
    })?;
    address_in(&bytes, &path, symbol)
}

/// The same, over bytes that are already in hand and a name to blame in the message.
///
/// Split out for the staging path, where a dropped ELF is read once and used twice -- as the
/// image via [`crate::elf::flatten`] and as the symbol table here. There is no file to re-read,
/// and `path` is the operator's own filename rather than somewhere on disk.
pub fn address_in(bytes: &[u8], path: &str, symbol: &str) -> Result<u32, SymbolError> {
    let path = path.to_owned();
    let file = object::File::parse(bytes).map_err(|err| SymbolError::NotAnElf {
        path: path.clone(),
        detail: err.to_string(),
    })?;

    // `symbols()` is `.symtab`, which is what a PlatformIO debug build leaves behind. A stripped
    // binary has only `.dynsym` -- irrelevant on a bare-metal Cortex-M, which has no dynamic
    // linking -- so a miss here is genuinely "the symbol is not there".
    let found = file
        .symbols()
        .find(|s| s.name().is_ok_and(|name| name == symbol))
        .ok_or_else(|| SymbolError::Missing {
            symbol: symbol.to_owned(),
            path: path.clone(),
        })?;

    let address = found.address();
    let size = found.size();

    // Exclusive at the top, unlike the stack pointer check in `device.rs`: a *variable* at
    // RAM_END would start one past the end of memory, whereas an initial SP legitimately points
    // there because the stack grows down from it.
    if !(u64::from(addr::RAM_BASE)..u64::from(addr::RAM_END)).contains(&address) {
        return Err(SymbolError::NotInRam {
            symbol: symbol.to_owned(),
            address,
        });
    }
    // Size 0 catches linker-script markers, which resolve fine and are not variables.
    if size != 4 || !address.is_multiple_of(4) {
        return Err(SymbolError::NotAWord {
            symbol: symbol.to_owned(),
            address,
            size,
        });
    }

    Ok(address as u32)
}

/// Build the smallest ELF32 little-endian ARM file with a symbol table in it.
///
/// Synthesised rather than checked in, because the thing being tested is the reading of a
/// *layout*, and a committed binary would only prove this can read one particular linker's
/// output. Building it here means each case differs by exactly the field under test.
///
/// Shared with `artefacts`, whose tests need a firmware.elf that actually resolves in order to
/// check that a loaded bundle carries a liveness address.
#[cfg(test)]
pub(crate) fn elf_with(symbols: &[(&str, u64, u64)]) -> Vec<u8> {
    const EHSIZE: usize = 52;
    const SHENTSIZE: usize = 40;
    const SYMSIZE: usize = 16;
    // Sections: null, .symtab, .strtab, .shstrtab.
    const SHNUM: usize = 4;

    let mut strtab = vec![0u8];
    let mut offsets = Vec::new();
    for (name, _, _) in symbols {
        offsets.push(strtab.len() as u32);
        strtab.extend_from_slice(name.as_bytes());
        strtab.push(0);
    }

    let mut shstrtab = vec![0u8];
    let symtab_name = shstrtab.len() as u32;
    shstrtab.extend_from_slice(b".symtab\0");
    let strtab_name = shstrtab.len() as u32;
    shstrtab.extend_from_slice(b".strtab\0");
    let shstrtab_name = shstrtab.len() as u32;
    shstrtab.extend_from_slice(b".shstrtab\0");

    // One null entry first, as ELF requires.
    let mut symtab = vec![0u8; SYMSIZE];
    for (index, (_, address, size)) in symbols.iter().enumerate() {
        symtab.extend_from_slice(&offsets[index].to_le_bytes());
        symtab.extend_from_slice(&(*address as u32).to_le_bytes());
        symtab.extend_from_slice(&(*size as u32).to_le_bytes());
        symtab.push(0x10); // GLOBAL, NOTYPE
        symtab.push(0); // other
        symtab.extend_from_slice(&1u16.to_le_bytes()); // shndx: anything but SHN_UNDEF
    }

    let symtab_off = EHSIZE;
    let strtab_off = symtab_off + symtab.len();
    let shstrtab_off = strtab_off + strtab.len();
    // The section header table has to be 4-aligned, and the string tables before it are
    // arbitrary lengths. A real linker pads here; forgetting to is rejected by the reader
    // with "Invalid ELF section header offset/size/alignment", which is a good sign that the
    // parsing being relied on is strict.
    let end_of_data = shstrtab_off + shstrtab.len();
    let sh_off = end_of_data.next_multiple_of(4);
    let padding = sh_off - end_of_data;

    let mut out = Vec::new();
    out.extend_from_slice(&[0x7F, b'E', b'L', b'F', 1, 1, 1, 0]); // 32-bit, LE, v1
    out.extend_from_slice(&[0; 8]);
    out.extend_from_slice(&1u16.to_le_bytes()); // ET_REL
    out.extend_from_slice(&40u16.to_le_bytes()); // EM_ARM
    out.extend_from_slice(&1u32.to_le_bytes()); // version
    out.extend_from_slice(&0u32.to_le_bytes()); // entry
    out.extend_from_slice(&0u32.to_le_bytes()); // phoff
    out.extend_from_slice(&(sh_off as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // flags
    out.extend_from_slice(&(EHSIZE as u16).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // phentsize
    out.extend_from_slice(&0u16.to_le_bytes()); // phnum
    out.extend_from_slice(&(SHENTSIZE as u16).to_le_bytes());
    out.extend_from_slice(&(SHNUM as u16).to_le_bytes());
    out.extend_from_slice(&3u16.to_le_bytes()); // shstrndx
    assert_eq!(out.len(), EHSIZE);

    out.extend_from_slice(&symtab);
    out.extend_from_slice(&strtab);
    out.extend_from_slice(&shstrtab);
    out.extend_from_slice(&vec![0u8; padding]);

    let mut section =
        |name: u32, kind: u32, offset: usize, size: usize, link: u32, entsize: u32, align: u32| {
            out.extend_from_slice(&name.to_le_bytes());
            out.extend_from_slice(&kind.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes()); // flags
            out.extend_from_slice(&0u32.to_le_bytes()); // addr
            out.extend_from_slice(&(offset as u32).to_le_bytes());
            out.extend_from_slice(&(size as u32).to_le_bytes());
            out.extend_from_slice(&link.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes()); // info
            out.extend_from_slice(&align.to_le_bytes()); // addralign
            out.extend_from_slice(&entsize.to_le_bytes());
        };
    section(0, 0, 0, 0, 0, 0, 0); // SHT_NULL
    section(
        symtab_name,
        2,
        symtab_off,
        symtab.len(),
        2,
        SYMSIZE as u32,
        4,
    ); // SHT_SYMTAB
    section(strtab_name, 3, strtab_off, strtab.len(), 0, 0, 1); // SHT_STRTAB
    section(shstrtab_name, 3, shstrtab_off, shstrtab.len(), 0, 0, 1);

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("portal-swd-{name}.elf"));
        let mut file = std::fs::File::create(&path).expect("temp file");
        file.write_all(bytes).expect("write");
        path
    }

    #[test]
    fn a_liveness_counter_resolves_to_its_address() {
        let path = write_temp(
            "ok",
            &elf_with(&[
                ("something_else", 0x2000_0100, 4),
                (LIVENESS_SYMBOL, 0x2000_0204, 4),
            ]),
        );
        assert_eq!(liveness_address(&path), Ok(0x2000_0204));
    }

    #[test]
    fn a_firmware_without_the_symbol_says_so_rather_than_guessing() {
        // The state of every build before this counter was added. It has to be a clear message,
        // because it is what an operator sees when they flash an older firmware.
        let path = write_temp("missing", &elf_with(&[("app", 0x2000_0100, 4)]));
        let err = liveness_address(&path).expect_err("should not resolve");
        assert!(matches!(err, SymbolError::Missing { .. }), "got {err:?}");
        assert!(err.to_string().contains("predates the run-check"));
    }

    #[test]
    fn a_symbol_in_flash_is_refused() {
        // A same-named *function* would resolve happily and never change, so the run-check would
        // poll a constant and report every board as dead.
        let path = write_temp("inflash", &elf_with(&[(LIVENESS_SYMBOL, 0x0800_6100, 4)]));
        assert_eq!(
            liveness_address(&path),
            Err(SymbolError::NotInRam {
                symbol: LIVENESS_SYMBOL.to_owned(),
                address: 0x0800_6100
            })
        );
    }

    #[test]
    fn a_zero_sized_marker_is_not_a_counter() {
        // Linker-script symbols resolve perfectly and are not variables.
        let path = write_temp("marker", &elf_with(&[(LIVENESS_SYMBOL, 0x2000_0200, 0)]));
        assert!(matches!(
            liveness_address(&path),
            Err(SymbolError::NotAWord { size: 0, .. })
        ));
    }

    #[test]
    fn a_misaligned_word_is_refused() {
        // The probe reads 32 bits at a time; an unaligned read on a Cortex-M0+ faults.
        let path = write_temp("odd", &elf_with(&[(LIVENESS_SYMBOL, 0x2000_0202, 4)]));
        assert!(matches!(
            liveness_address(&path),
            Err(SymbolError::NotAWord { .. })
        ));
    }

    #[test]
    fn a_missing_file_is_reported_by_path() {
        let err = liveness_address(Path::new("does-not-exist.elf")).expect_err("should fail");
        assert!(matches!(err, SymbolError::Unreadable { .. }), "got {err:?}");
        assert!(err.to_string().contains("does-not-exist.elf"));
    }

    /// The one test here that reads a real linker's output.
    ///
    /// Everything above is synthesised, which is right for testing the *reading* of a layout and
    /// proves nothing about the layout PlatformIO's arm-none-eabi actually emits. Skipped rather
    /// than failed when PortalFW has not been built, because a fresh clone has not.
    #[test]
    fn the_real_portalfw_build_resolves_if_it_has_been_built() {
        let elf = crate::artefacts::repo_root()
            .join("PortalFW/.pio/build/application_bank_optical/firmware.elf");
        if !elf.is_file() {
            eprintln!("skipping: PortalFW has not been built here");
            return;
        }

        let address = liveness_address(&elf).expect("g_liveness_counter should resolve");
        // Deliberately not asserting the value: it is a linker output and moves with any edit
        // that shifts `.bss`, which is the entire reason it is read rather than written down.
        // What is asserted is everything the run-check depends on.
        assert!(
            (addr::RAM_BASE..addr::RAM_END).contains(&address),
            "{address:#010X} should be in RAM"
        );
        assert_eq!(address % 4, 0, "the probe reads it as an aligned word");
    }

    #[test]
    fn a_file_that_is_not_an_elf_is_reported_as_such() {
        let path = write_temp("notelf", b"this is a .bin, not a .elf");
        let err = liveness_address(&path).expect_err("should fail");
        assert!(matches!(err, SymbolError::NotAnElf { .. }), "got {err:?}");
    }
}
