//! Write a bootloader and/or an application to the board on the probe, over SWD.
//!
//! ```text
//! cargo run --example flash_board --features probe -- --list
//! cargo run --example flash_board --features probe -- --bootloader <id> --application <id>
//! cargo run --example flash_board --features probe -- --application <id> [--erase-unselected]
//! ```
//!
//! Everything about *which* image may go *where* is decided by [`ImageBundle::validate`], which is
//! also what the bench and the GUI use. This file only chooses artefacts and prints what happened.
//!
//! # Why a pass should carry both regions
//!
//! The bank boundary comes from the application's own load address, so a pass carrying an
//! application writes the bootloader bank all the way up to it. A bootloader-only pass has no
//! boundary to take and falls back to whichever bank its own image needs — pages 0-7 for a v6
//! image — which deliberately leaves pages 8-11 alone, because that pass cannot know what is on
//! the board.
//!
//! That matters when replacing a *longer* bootloader with a shorter one. The fielded v5 is 19,568
//! bytes and runs to `0x08004C70`, i.e. into pages 8-9. Install v6 by itself and its tail stays
//! behind; v6 then finds its application bank not blank, fails the vector-table and descriptor
//! checks on those leftover bytes, and refuses to start the perfectly good application still
//! sitting at `0x08006000`. The board is reachable and recoverable — but it will not run, and
//! nothing about the symptom points at the cause.
//!
//! Pair the two, and the boundary moves to the application's base and the tail is erased with the
//! rest of the bank.

#![cfg(feature = "probe")]

use portal_swd::artefacts::{discover, Selection};
use portal_swd::image::Unselected;
use portal_swd::rig::{Release, Rig, Step};
use portal_swd::ProbeRsRig;

fn main() {
    if let Err(err) = run() {
        eprintln!("\nFAILED: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut bootloader: Option<String> = None;
    let mut application: Option<String> = None;
    let mut unselected = Unselected::Preserve;
    let mut list = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bootloader" => bootloader = args.next(),
            "--application" => application = args.next(),
            "--erase-unselected" => unselected = Unselected::Erase,
            "--list" => list = true,
            other => return Err(format!("unknown argument '{other}'").into()),
        }
    }

    let discovery = discover();
    if list || (bootloader.is_none() && application.is_none()) {
        println!("artefacts found under {}:\n", portal_swd::artefacts::artefact_root().display());
        for artefact in discovery.bootloader().into_iter() {
            print_artefact(artefact);
        }
        for artefact in discovery.applications() {
            print_artefact(artefact);
        }
        if !list {
            println!("\nnothing selected; pass --bootloader <id> and/or --application <id>");
        }
        return Ok(());
    }

    let selection = Selection {
        bootloader,
        application,
    };
    let bundle = discovery
        .load(&selection, unselected)
        .map_err(|err| format!("{err:?}"))?;

    // Refuses the pairings that cannot work before the probe is even opened: a bootloader too
    // large for its bank, an application at an address nothing knows how to start, and the pair
    // rule that catches a 24 kB bootloader sitting on top of a `0x08004000` application.
    let faults = bundle.validate();
    if !faults.is_empty() {
        for fault in &faults {
            eprintln!("  refused: {fault:?}");
        }
        return Err("the bundle is not safe to write".into());
    }

    println!("scope        {}", selection.scope());
    println!(
        "bootloader   {} bytes at 0x{:08X}",
        bundle.bootloader.bytes.len(),
        bundle.bootloader.load_address
    );
    println!(
        "application  {} bytes at 0x{:08X}",
        bundle.application.bytes.len(),
        bundle.application.load_address
    );
    for window in bundle.write_windows() {
        println!(
            "write window 0x{:08X}..0x{:08X}  ({} bytes)",
            window.start,
            window.end(),
            window.bytes.len()
        );
    }
    for (start, end) in bundle.preserved_windows() {
        println!("preserved    0x{start:08X}..0x{end:08X}  (read before, compared after)");
    }
    println!("run check    VTOR must read 0x{:08X}\n", bundle.run_check.vtor);

    let mut rig = ProbeRsRig::new(None);
    let info = rig.open().map_err(|err| format!("{err:?}"))?;
    println!("probe        {} {}", info.name, info.serial.as_deref().unwrap_or("-"));

    let mut last = String::new();
    let mut progress = |step: Step, done: u64, total: u64| {
        let label = format!("{step:?}");
        if label != last {
            println!("  {label} ({done}/{total})");
            last = label;
        }
    };
    let report = rig
        .flash(&bundle, Release::Run, &mut progress)
        .map_err(|err| format!("{err:?}"))?;
    println!("\n{report:#?}");
    Ok(())
}

fn print_artefact(artefact: &portal_swd::artefacts::Artefact) {
    println!(
        "  {:<44} {:>7} B  0x{:08X}  {}",
        artefact.id,
        artefact.bytes,
        artefact.base,
        artefact.banner.as_deref().unwrap_or("(no banner)")
    );
}
