//! Does the probe work at all, and are the assumptions in the plan true?
//!
//! Run with a board on the pogo pins:
//!
//! ```text
//! cargo run --example probe_spike
//! ```
//!
//! This deliberately does not use `Rig`, `Machine` or anything else in the crate. It is the
//! smallest possible program that answers the questions the rest of the design rests on, so that
//! a wrong answer costs one file rather than a rewrite:
//!
//! 1. Does the ST-Link enumerate, and what firmware does it report?
//! 2. Can a **non-invasive** read work — `attach_to_unspecified` then a raw ARM interface, with
//!    no `Session`, no halt and no reset?
//! 3. Is `RCC_APBENR1.DBGEN` really 0 out of reset? The plan says the cheap poll must not read
//!    `DBGMCU_IDCODE` because of it, but that is inferred from probe-rs's source rather than from
//!    RM0454. If IDCODE reads correctly here, the poll can be simpler than planned.
//! 4. Does the 128 kB readback work, and how long does it take?
//!
//! It writes nothing to the target.

#![cfg(feature = "probe")]

use std::time::Instant;

use probe_rs::architecture::arm::{
    FullyQualifiedApAddress, dp::DpAddress, sequences::DefaultArmSequence,
};
use probe_rs::probe::list::Lister;

use portal_swd::{DeviceImage, addr};

fn main() {
    if let Err(err) = run() {
        eprintln!("\nFAILED: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    // ---- 1. enumeration
    let lister = Lister::new();
    let probes = lister.list_all();
    println!("probes found: {}", probes.len());
    for (index, info) in probes.iter().enumerate() {
        println!(
            "  [{index}] {}  {:04x}:{:04x}  serial={:?}  kind={}",
            info.identifier,
            info.vendor_id,
            info.product_id,
            info.serial_number.as_deref().unwrap_or("-"),
            info.probe_type()
        );
    }
    let Some(info) = probes.first() else {
        return Err("no probes. Is the ST-Link plugged in?".into());
    };

    // `DebugProbeInfo::open()` rather than `Lister::open(selector)`: no string round-trip, and
    // the not-found case cannot happen.
    let mut probe = info.open()?;
    println!("\nopened: {}", probe.get_name());
    println!("  speed: {} kHz", probe.speed_khz());
    match probe.get_target_voltage() {
        Ok(Some(v)) => println!("  vtarget: {v:.2} V"),
        Ok(None) => println!("  vtarget: not reported by this probe"),
        Err(err) => println!("  vtarget: error ({err})"),
    }

    // ---- 2. the non-invasive path
    //
    // `try_into_arm_debug_interface` returns NotAttached unless this runs first. That is not
    // visible in its signature and is the single most likely thing to waste an afternoon.
    probe.attach_to_unspecified()?;

    let mut iface = match probe.try_into_arm_debug_interface(DefaultArmSequence::create()) {
        Ok(iface) => iface,
        Err((_probe, err)) => return Err(format!("could not open the ARM interface: {err}").into()),
    };

    let connect = Instant::now();
    iface.select_debug_port(DpAddress::Default)?;
    println!("\nselect_debug_port: {:?}", connect.elapsed());

    let ap = FullyQualifiedApAddress::v1_with_default_dp(0);
    let mut mem = iface.memory_interface(&ap)?;

    // ---- the identity reads, none of which need a peripheral clock
    let mut uid = [0u32; 3];
    mem.read_32(u64::from(addr::UID_BASE), &mut uid)?;
    let flash_kb = mem.read_word_16(u64::from(addr::FLASHSIZE_BASE))?;
    println!("UID        : {:08X}-{:08X}-{:08X}", uid[0], uid[1], uid[2]);
    println!("flash size : {flash_kb} kB");

    // ---- 3. the DBGEN question
    let apbenr1 = mem.read_word_32(u64::from(addr::RCC_APBENR1))?;
    let dbgen = apbenr1 & (1 << 27) != 0; // RCC_APBENR1.DBGEN
    let idcode = mem.read_word_32(u64::from(addr::DBGMCU_IDCODE)).ok();
    println!("\nRCC_APBENR1: {apbenr1:#010X}  (DBGEN bit 27 = {dbgen})");
    match &idcode {
        Some(value) => println!(
            "DBGMCU_IDCODE: {value:#010X}  DEV_ID={:#05X} REV_ID={:#06X}",
            value & 0xFFF,
            value >> 16
        ),
        None => println!("DBGMCU_IDCODE: unreadable -- as the plan predicted"),
    }
    let optr = mem.read_word_32(u64::from(addr::FLASH_OPTR))?;
    let rcc_csr = mem.read_word_32(u64::from(addr::RCC_CSR))?;
    println!("FLASH_OPTR : {optr:#010X}");
    println!("RCC_CSR    : {rcc_csr:#010X}");

    // ---- 4. the readback
    let span = (addr::FLASH_END - addr::FLASH_BASE) as usize;
    let mut image = vec![0u8; span];
    let started = Instant::now();
    mem.read(u64::from(addr::FLASH_BASE), &mut image)?;
    let elapsed = started.elapsed();

    println!(
        "\nreadback   : {} bytes in {:?} ({:.0} kB/s)",
        image.len(),
        elapsed,
        image.len() as f64 / 1024.0 / elapsed.as_secs_f64()
    );

    // Everything below comes from the crate's own model rather than from this file, so a real
    // board is what validates `DeviceImage::analyse` rather than a fixture agreeing with itself.
    let report = DeviceImage {
        flash: image,
        optr,
        idcode,
        uid,
        flash_kb,
        rcc_csr,
    }
    .analyse();

    println!(
        "programmed : {} bytes ({:.1}%)",
        report.programmed_bytes,
        100.0 * report.programmed_bytes as f64 / report.total_bytes as f64
    );
    println!("\nlayout     : {}", report.layout.as_str());
    if let Some(v) = report.flat_vector {
        println!(
            "             one image, SP={:#010X} entry={:#010X} -- no bootloader, so RS485 \
             field update cannot reach it",
            v.initial_sp,
            v.entry()
        );
    }
    for region in [&report.bootloader, &report.application] {
        if region.is_erased() {
            println!("{:11}: erased", region.name);
        } else {
            println!(
                "{:11}: {} bytes, vector {}, sha {}",
                region.name,
                region.used_bytes,
                if region.vector.is_some() {
                    "ok"
                } else {
                    "none"
                },
                &region.sha256[..12]
            );
        }
        if let Some(banner) = &region.banner {
            println!("{:11}:   banner {banner:?}", region.name);
        }
    }

    let o = report.options;
    println!(
        "\noptions    : {:#010X}  RDP L{}  IWDG_SW={}  nBOOT_SEL={}  NRST_MODE={:#04b}",
        o.raw,
        o.rdp_level(),
        o.iwdg_sw as u8,
        o.nboot_sel as u8,
        o.nrst_mode
    );
    if o.warnings().is_empty() {
        println!("             nothing unsafe about working on this board");
    }
    for warning in o.warnings() {
        println!("  WARNING  : {warning}");
    }

    drop(mem);
    let _probe = iface.close();
    println!("\ndetached cleanly. Nothing was written.");
    Ok(())
}
