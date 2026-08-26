//! Update an RS485 repeater's firmware in band, from the command line.
//!
//! The sequencing lives in [`router_link::repeater_ota::run_update`] and this is a CLI
//! over it. It used to live *here*, which meant the PortalTestBench could not reach it and
//! would have had to grow a second copy — and three of its invariants each cost a bench
//! session to find. One implementation, exercised two ways:
//!
//! ```text
//!   ota-begin   -- unicast, acknowledged; the erase blocks and drops inbound bytes, so
//!                  nothing may be streamed until every target has answered
//!   ota-data*   -- unacknowledged stream, every chunk once; broadcast when more than one
//!                  repeater is being updated at a time
//!   ota-map     -- unicast; which chunks actually landed
//!   ota-data*   -- exactly the gaps, repeated until every map is full
//!   ota-end     -- unicast; SHA-256 over the written slot, then commit
//!   ota-boot    -- reboot into the new slot
//!   ota-confirm -- unicast; mark the new image good, so it is not rolled back
//! ```
//!
//! ```text
//!   flash_repeater --port <usb-serial-number> <firmware.bin> [--index N[,N...]]
//!                  [--chunk 512] [--gap 2] [--no-boot] [--dry-run]
//!   flash_repeater --port <usb-serial-number> status [--index N]
//!   flash_repeater --port <usb-serial-number> abort  [--index N]
//!   flash_repeater --port <usb-serial-number> probe  [--first N] [--keep]
//! ```
//!
//! `--index` takes a comma-separated list. More than one is a **broadcast** update: the
//! data pass feeds every named repeater at once, which is faster and blacks out every one
//! of their branches for the duration. `Protocol.md` §12 keeps `ota-begin`, `ota-map`,
//! `ota-end` and `ota-confirm` unicast regardless, so this is N sessions sharing a stream.
//!
//! The port is named by USB serial number, not `/dev/cu.*` path, for the same reason
//! `flash_portals` does: several serial functions are attached at once and the node
//! numbers move.

use std::time::{Duration, Instant};

use router_link::repeater_ota::{
    self as ota, OtaObserver, OtaPhase, RepeaterImage, RepeaterOtaParams,
};
use router_link::rs485::{device::SerialPortDevice, Rs485};
use router_proto::repeater::{RepeaterTarget, RepeaterVerb};

const USAGE: &str = "\
usage:
  flash_repeater --port <usb-serial-number> <firmware.bin> [--index N[,N...]] [--chunk 512]
                                                           [--gap 2] [--no-boot] [--dry-run]
  flash_repeater --port <usb-serial-number> status [--index N]
  flash_repeater --port <usb-serial-number> abort  [--index N]
  flash_repeater --port <usb-serial-number> probe  [--first N] [--keep]";

fn find_port(serial_number: &str) -> Option<String> {
    serialport::available_ports()
        .ok()?
        .into_iter()
        .find_map(|port| match &port.port_type {
            serialport::SerialPortType::UsbPort(info)
                if info.serial_number.as_deref() == Some(serial_number) =>
            {
                Some(port.port_name)
            }
            _ => None,
        })
}

struct Args {
    positional: Vec<String>,
    flags: std::collections::HashMap<String, Option<String>>,
}

impl Args {
    fn parse() -> Self {
        const VALUED: [&str; 5] = ["--port", "--index", "--chunk", "--gap", "--first"];
        let mut positional = Vec::new();
        let mut flags = std::collections::HashMap::new();
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            if arg.starts_with("--") {
                let value = if VALUED.contains(&arg.as_str()) {
                    args.next()
                } else {
                    None
                };
                flags.insert(arg, value);
            } else {
                positional.push(arg);
            }
        }
        Self { positional, flags }
    }

    fn value(&self, name: &str) -> Option<&str> {
        self.flags.get(name)?.as_deref()
    }

    fn present(&self, name: &str) -> bool {
        self.flags.contains_key(name)
    }

    fn params(&self, default_gap: &str) -> Result<RepeaterOtaParams, Box<dyn std::error::Error>> {
        Ok(RepeaterOtaParams {
            chunk_bytes: self.value("--chunk").unwrap_or("512").parse()?,
            wait_between_chunks_ms: self.value("--gap").unwrap_or(default_gap).parse()?,
            ..RepeaterOtaParams::default()
        })
    }

    fn targets(&self) -> Result<Vec<RepeaterTarget>, Box<dyn std::error::Error>> {
        let mut targets = Vec::new();
        for part in self.value("--index").unwrap_or("1").split(',') {
            targets.push(ota::validate_index(part.trim().parse()?)?);
        }
        Ok(targets)
    }
}

/// One line per phase change, and a carriage return within one.
///
/// The driver reports at about 50 Hz during a stream; a terminal wants far less than that,
/// and a log file wants one line per thing that happened rather than one per frame.
struct PrintObserver {
    phase: Option<OtaPhase>,
    started: Instant,
    last_draw: Instant,
}

impl PrintObserver {
    fn new() -> Self {
        Self {
            phase: None,
            started: Instant::now(),
            last_draw: Instant::now() - Duration::from_secs(1),
        }
    }
}

impl OtaObserver for PrintObserver {
    fn phase(&mut self, phase: OtaPhase, fraction: f32, detail: &str) {
        use std::io::Write;
        let fresh = self.phase != Some(phase);
        if fresh {
            if self.phase.is_some() {
                println!();
            }
            self.phase = Some(phase);
        } else if self.last_draw.elapsed() < Duration::from_millis(250) {
            return;
        }
        self.last_draw = Instant::now();
        print!(
            "\r{:<8} {:>3}%  {detail}  ({:.0}s)      ",
            phase.as_str(),
            (fraction * 100.0).round() as u32,
            self.started.elapsed().as_secs_f32()
        );
        std::io::stdout().flush().ok();
    }
}

fn open_bus(serial_number: &str) -> Result<Rs485, Box<dyn std::error::Error>> {
    let port = find_port(serial_number)
        .ok_or_else(|| format!("no USB serial device with serial number {serial_number}"))?;
    println!("port     {port}  (serial {serial_number})");
    let mut rs485 = Rs485::new(0, router_report::Reporter::disabled());
    rs485.open_device(Box::new(SerialPortDevice::open(&port)?));
    rs485.update();
    Ok(rs485)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let Some(first) = args.positional.first().cloned() else {
        println!("{USAGE}");
        return Ok(());
    };
    let serial_number = args
        .value("--port")
        .ok_or("--port <usb-serial-number> is required")?
        .to_string();
    let targets = args.targets()?;

    match first.as_str() {
        "status" => {
            let mut rs485 = open_bus(&serial_number)?;
            for target in &targets {
                match ota::read_status(&mut rs485, target, Duration::from_secs(3)) {
                    Ok(reply) => println!("{reply:#?}"),
                    Err(error) => println!("{error}"),
                }
            }
            rs485.close();
            Ok(())
        }
        "abort" => {
            let mut rs485 = open_bus(&serial_number)?;
            for target in &targets {
                ota::abort(&rs485, target);
            }
            ota::drain(
                &mut rs485,
                Instant::now(),
                Duration::ZERO,
                Duration::from_secs(3),
                Duration::ZERO,
            );
            println!("abort sent to {} repeater(s)", targets.len());
            rs485.close();
            Ok(())
        }
        "probe" => probe(&args, &serial_number, &targets),
        path => flash(path, &args, &serial_number, &targets),
    }
}

/// Opens a session, sends exactly `--first N` chunks of a synthetic image, and reads the
/// map back. Separates "the repeater rejects the frame" from "the frames are not arriving":
/// at one or two chunks there is no throughput to blame.
fn probe(
    args: &Args,
    serial_number: &str,
    targets: &[RepeaterTarget],
) -> Result<(), Box<dyn std::error::Error>> {
    let count: usize = args.value("--first").unwrap_or("1").parse()?;
    let params = args.params("50")?;
    let target = targets.first().ok_or("no repeater named")?;
    // Synthetic, but the same shape as a real image: enough chunks that indices are
    // meaningful, and a byte pattern that includes zeros so COBS has real work to do.
    let bytes: Vec<u8> = (0..params.chunk_bytes * count.max(8))
        .map(|i| (i % 251) as u8)
        .collect();
    let image = RepeaterImage::new(bytes, params.chunk_bytes)?;

    let mut rs485 = open_bus(serial_number)?;
    ota::begin(&rs485, target, &image, &params);
    let Some(reply) = ota::await_reply(
        &mut rs485,
        RepeaterVerb::OtaBegin,
        Duration::from_millis(params.begin_timeout_ms as u64),
    ) else {
        return Err("the repeater did not answer ota-begin".into());
    };
    println!("begin    ok={} payload={:?}", reply.ok, reply.payload);

    let indices: Vec<usize> = (0..count.min(image.chunk_count())).collect();
    let started = Instant::now();
    let queued = ota::send_chunks(&rs485, target, &image, &params, &indices);
    let wire = ota::wire_time(queued, &params);
    ota::drain(
        &mut rs485,
        started,
        wire,
        wire * 3 + Duration::from_secs(30),
        Duration::from_millis(params.settle_after_burst_ms as u64),
    );
    println!("stream   {queued} chunk(s) of {} bytes sent", params.chunk_bytes);

    ota::request_map(&rs485, target, &params);
    match ota::await_reply(&mut rs485, RepeaterVerb::OtaMap, Duration::from_secs(6)) {
        Some(reply) => println!("map      {:?}", reply.payload),
        None => println!("map      no answer"),
    }
    if !args.present("--keep") {
        ota::abort(&rs485, target);
        ota::drain(
            &mut rs485,
            Instant::now(),
            Duration::ZERO,
            Duration::from_secs(3),
            Duration::ZERO,
        );
    }
    rs485.close();
    Ok(())
}

fn flash(
    path: &str,
    args: &Args,
    serial_number: &str,
    targets: &[RepeaterTarget],
) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = std::fs::read(path)?;
    let params = args.params("2")?;
    let image = RepeaterImage::new(bytes, params.chunk_bytes)?;

    println!("image    {path}  ({} bytes)", image.len());
    println!(
        "chunks   {} x {} bytes,  SHA-256 {}",
        image.chunk_count(),
        params.chunk_bytes,
        hex(image.sha256())
    );
    println!(
        "targets  {}{}",
        targets
            .iter()
            .map(ota::describe_target)
            .collect::<Vec<_>>()
            .join(", "),
        if targets.len() > 1 {
            "   (BROADCAST data pass: every one of these branches goes dark for the duration)"
        } else {
            ""
        }
    );
    println!(
        "estimate {:.0}s of wire time\n",
        image.estimated_seconds(&params)
    );
    if args.present("--dry-run") {
        return Ok(());
    }

    let mut rs485 = open_bus(serial_number)?;
    let mut observer = PrintObserver::new();
    let report = ota::run_update(
        &mut rs485,
        targets,
        &image,
        &params,
        !args.present("--no-boot"),
        &mut observer,
    );
    rs485.close();
    let report = report?;

    println!("\n");
    println!(
        "done     {} chunks in {:.0}s, {} repair round(s), {} chunk(s) re-sent",
        report.chunks, report.seconds, report.repair_rounds, report.repaired_chunks
    );
    if args.present("--no-boot") {
        println!("         committed but not booted (--no-boot); send ota-boot when ready");
    } else {
        for entry in &report.targets {
            // Not fatal: an unconfirmed image resolves itself on local evidence in about
            // 30 seconds. Worth saying out loud, though, because until then a power cut
            // rolls the repeater back.
            println!(
                "         repeater {}: {}",
                ota::describe_target(&entry.target),
                if entry.confirmed {
                    "confirmed"
                } else {
                    "not confirmed -- it will self-confirm in ~30s"
                }
            );
        }
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
