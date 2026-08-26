//! Save the whole 128 kB of a board's flash to a file, writing nothing to the target.
//!
//! ```text
//! cargo run --example readback --features probe -- /path/to/board.bin
//! ```
//!
//! Exists because the interesting part of a board is not reproducible from the repo: the
//! identity record and the two settings journals in the last three pages are per-board and
//! are what a legacy bootloader update destroys. Capturing them before an update is the
//! only way back.

#![cfg(feature = "probe")]

use probe_rs::architecture::arm::{
    FullyQualifiedApAddress, dp::DpAddress, sequences::DefaultArmSequence,
};
use probe_rs::probe::list::Lister;

use portal_swd::addr;

fn main() {
    if let Err(err) = run() {
        eprintln!("\nFAILED: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: readback <output.bin>")?;

    let lister = Lister::new();
    let probes = lister.list_all();
    let info = probes.first().ok_or("no probes. Is the ST-Link plugged in?")?;
    println!("probe      {}  serial={:?}", info.identifier, info.serial_number);

    let mut probe = info.open()?;
    // Required before `try_into_arm_debug_interface`, which otherwise returns NotAttached
    // without saying so in its signature.
    probe.attach_to_unspecified()?;
    let mut iface = probe
        .try_into_arm_debug_interface(DefaultArmSequence::create())
        .map_err(|(_, error)| format!("could not open the ARM interface: {error}"))?;
    iface.select_debug_port(DpAddress::Default)?;
    let ap = FullyQualifiedApAddress::v1_with_default_dp(0);
    let mut memory = iface.memory_interface(&ap)?;

    let mut flash = vec![0u8; (addr::FLASH_END - addr::FLASH_BASE) as usize];
    memory.read(addr::FLASH_BASE as u64, &mut flash)?;

    // The UID is what ties this image to the board it came off; without it a file of
    // 131072 bytes is indistinguishable from any other board's.
    let mut uid = [0u8; 12];
    memory.read(addr::UID_BASE as u64, &mut uid)?;
    let uid_hex: String = uid.iter().map(|b| format!("{b:02x}")).collect();

    std::fs::write(&path, &flash)?;
    println!("uid        {uid_hex}");
    println!("wrote      {} bytes to {path}", flash.len());

    let persist = &flash[(addr::PERSIST_BASE - addr::FLASH_BASE) as usize..];
    let blank = persist.iter().all(|b| *b == 0xFF);
    println!(
        "persistent {} bytes, {}",
        persist.len(),
        if blank { "blank" } else { "populated" }
    );
    Ok(())
}
