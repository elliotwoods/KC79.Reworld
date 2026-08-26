//! What is the board actually running, right now, without disturbing it?
//!
//! ```text
//! cargo run --example observe --features probe [-- --watch 20]
//! ```
//!
//! Reads `SCB->VTOR` and the core's debug status. Between them they answer the one question a
//! flash readback cannot: a board can hold a perfect image and still not be running it.
//!
//! - `VTOR == 0x20000000` — bootloader v6 is resident. It relocates its vectors into SRAM before
//!   enabling an interrupt, so this, not zero, is what a running v6 looks like.
//! - `VTOR == 0` — a v4/v5 bootloader is resident and executing.
//! - `VTOR == 0x08006000` — the bootloader handed over to a legacy-base application.
//! - `VTOR == 0x08004000` — it handed over to a new-base one.
//! - lockup with `VTOR` at an application base — it jumped to a bank with no image in it.
//!
//! Writes nothing, halts nothing, resets nothing.

#![cfg(feature = "probe")]

use std::time::{Duration, Instant};

use probe_rs::architecture::arm::{
    FullyQualifiedApAddress, dp::DpAddress, sequences::DefaultArmSequence,
};
use probe_rs::probe::list::Lister;

/// Cortex-M vector table offset register.
const SCB_VTOR: u64 = 0xE000_ED08;
/// Debug Halting Control and Status Register: `S_LOCKUP` is bit 19, `S_HALT` bit 17.
const DHCSR: u64 = 0xE000_EDF0;

fn main() {
    if let Err(err) = run() {
        eprintln!("\nFAILED: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut watch_secs = 0u64;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--watch" => watch_secs = args.next().unwrap_or_default().parse()?,
            other => return Err(format!("unknown argument '{other}'").into()),
        }
    }

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

    let started = Instant::now();
    let mut last = String::new();
    loop {
        let vtor = memory.read_word_32(SCB_VTOR)?;
        let dhcsr = memory.read_word_32(DHCSR)?;
        let line = format!(
            "VTOR 0x{vtor:08X}  {:<28} halted={} lockup={}",
            describe(vtor),
            (dhcsr & (1 << 17)) != 0,
            (dhcsr & (1 << 19)) != 0,
        );
        if line != last {
            println!("{:>6.1}s  {line}", started.elapsed().as_secs_f32());
            last = line;
        }
        if started.elapsed() >= Duration::from_secs(watch_secs) {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Ok(())
}

fn describe(vtor: u32) -> &'static str {
    match vtor {
        // v6 relocates its vector table into SRAM before enabling any interrupt, so a resident
        // v6 reads 0x20000000 here and a resident v4/v5 reads 0 or the flash base.
        0x2000_0000 => "bootloader v6 resident",
        0x0000_0000 | 0x0800_0000 => "bootloader resident (v4/v5)",
        0x0800_4000 => "application (new base)",
        0x0800_6000 => "application (legacy base)",
        _ => "unexpected",
    }
}
