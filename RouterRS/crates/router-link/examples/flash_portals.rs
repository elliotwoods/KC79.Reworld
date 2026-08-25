//! Broadcast a PortalFW application image to every module on one RS485 bus.
//!
//! Drives `router_link::fw_update` rather than re-implementing the sequence: the
//! magic words, the `frameOffset`/`frame_size` pairing and the pacing there are
//! golden-tested against the fielded bootloader, and the one historical bug in
//! this path (advancing the offset by the GUI frame size while sending a
//! hardcoded 32 bytes) is already fixed in it.
//!
//! Firmware update is broadcast-only, so overlapping module IDs do not matter —
//! every board in earshot takes the image regardless of its address.
//!
//!     cargo run -p router-link --example flash_portals -- <serial-number> <image.bin>
//!
//! The port is named by USB serial number, never by `/dev/cu.*` path: the node
//! number changes across reconnects, and this repo has three different serial
//! functions that can be attached at once.

use std::io::Write;
use std::time::{Duration, Instant};

use router_link::fw_update::{self, FwUpdateParams};
use router_link::rs485::{device::SerialPortDevice, Rs485};

fn find_port(serial_number: &str) -> Option<String> {
    serialport::available_ports().ok()?.into_iter().find_map(|port| {
        match &port.port_type {
            serialport::SerialPortType::UsbPort(info)
                if info.serial_number.as_deref() == Some(serial_number) =>
            {
                Some(port.port_name)
            }
            _ => None,
        }
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let serial_number = args.next().ok_or("usage: flash_portals <serial-number> <image.bin>")?;
    let image_path = args.next().ok_or("usage: flash_portals <serial-number> <image.bin>")?;

    let image = std::fs::read(&image_path)?;
    let port = find_port(&serial_number)
        .ok_or_else(|| format!("no USB serial device with serial number {serial_number}"))?;

    // The multi-board profile: 10 ms between frames and six repetitions. The
    // bootloader requires strictly sequential offsets and cannot ask for a resend,
    // so a single dropped frame ends that board's upload -- repetition is the only
    // recovery, and it is free because a duplicate offset is silently skipped.
    let params = FwUpdateParams::mass();

    println!("port     {port}  (serial {serial_number})");
    println!("image    {image_path}  ({} bytes)", image.len());
    println!(
        "profile  {} B frames, {} ms apart, x{} repetitions",
        params.frame_size, params.wait_between_frames_ms, params.frame_repetitions
    );

    let reporter = router_report::Reporter::disabled();
    let mut rs485 = Rs485::new(0, reporter);
    rs485.open_device(Box::new(SerialPortDevice::open(&port)?));
    rs485.update();
    if !rs485.is_connected() {
        // The worker sets this on its first successful poll; give it a moment.
        std::thread::sleep(Duration::from_millis(200));
        rs485.update();
    }

    let queued = fw_update::upload(&rs485, &image, &params)?;
    println!("\nqueued   {queued} packets\n");

    let started = Instant::now();
    let mut last_report = Instant::now();
    loop {
        rs485.update();
        let remaining = rs485.outbox_len();
        if remaining == 0 {
            break;
        }
        if last_report.elapsed() >= Duration::from_secs(5) {
            let sent = queued.saturating_sub(remaining);
            let percent = 100.0 * sent as f32 / queued as f32;
            print!(
                "\r  {sent}/{queued} packets ({percent:.1}%)  {:.0}s elapsed   ",
                started.elapsed().as_secs_f32()
            );
            std::io::stdout().flush().ok();
            last_report = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    println!("\r  {queued}/{queued} packets (100.0%)  {:.0}s elapsed   ", started.elapsed().as_secs_f32());

    // "RU" is deliberately separate from upload(): it ends the bootloader's
    // residency window and jumps to the application.
    fw_update::run_application(&rs485, &params);
    while rs485.outbox_len() > 0 {
        rs485.update();
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(500));
    rs485.update();

    let stats = rs485.stats();
    println!(
        "\nsent {} frames, received {}, decode errors {}",
        stats.tx_count, stats.rx_count, stats.decode_errors
    );
    println!("run application sent; boards should be executing the new image");
    rs485.close();
    Ok(())
}
