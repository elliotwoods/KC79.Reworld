//! Reset the target through SWD, without flashing or halting it.
//!
//! ```text
//! cargo run --example reset --features probe
//! ```
//!
//! Writes `SCB->AIRCR = VECTKEY | SYSRESETREQ` and nothing else. Useful when a board has
//! stopped answering on the bus but is otherwise healthy: it is the one way to restart it
//! that does not involve reaching for the power.

#![cfg(feature = "probe")]

use probe_rs::architecture::arm::{
    FullyQualifiedApAddress, dp::DpAddress, sequences::DefaultArmSequence,
};
use probe_rs::probe::list::Lister;

/// Cortex-M System Control Block, Application Interrupt and Reset Control Register.
const SCB_AIRCR: u64 = 0xE000_ED0C;
/// `VECTKEY` (0x05FA) in the top half; `SYSRESETREQ` is bit 2. Any other key is ignored.
const AIRCR_SYSRESETREQ: u32 = 0x05FA_0004;

fn main() {
    if let Err(err) = run() {
        eprintln!("\nFAILED: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let lister = Lister::new();
    let probes = lister.list_all();
    let info = probes.first().ok_or("no probes. Is the ST-Link plugged in?")?;

    let mut probe = info.open()?;
    probe.attach_to_unspecified()?;
    let mut iface = probe
        .try_into_arm_debug_interface(DefaultArmSequence::create())
        .map_err(|(_, error)| format!("could not open the ARM interface: {error}"))?;
    iface.select_debug_port(DpAddress::Default)?;
    let ap = FullyQualifiedApAddress::v1_with_default_dp(0);
    let mut memory = iface.memory_interface(&ap)?;

    // The write itself is expected to be the last thing the core acknowledges, so an error
    // here usually means the reset worked rather than that it failed.
    match memory.write_word_32(SCB_AIRCR, AIRCR_SYSRESETREQ) {
        Ok(()) => println!("reset requested"),
        Err(error) => println!("reset requested (the probe reported {error}, which is normal)"),
    }
    Ok(())
}
