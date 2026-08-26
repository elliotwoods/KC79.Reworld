//! Update an RS485 repeater's firmware in band, from the command line.
//!
//! [`router_link::repeater_ota`] provides the individual steps; the sequencing between
//! them is genuinely reply-driven, so it lives here rather than in the router runtime's
//! fire-and-forget command handler:
//!
//! ```text
//!   ota-begin   -- acknowledged; the erase blocks and drops inbound bytes, so nothing
//!                  may be streamed until it answers
//!   ota-data*   -- unacknowledged stream, every chunk once
//!   ota-map     -- which chunks actually landed
//!   ota-data*   -- exactly the gaps, repeated until the map is full
//!   ota-end     -- SHA-256 over the written slot, then commit
//!   ota-boot    -- reboot into the new slot
//!   ota-confirm -- mark the new image good, so it is not rolled back
//! ```
//!
//! ```text
//!   flash_repeater --port <usb-serial-number> <firmware.bin> [--index N] [--chunk 512]
//!                  [--no-boot] [--dry-run]
//!   flash_repeater --port <usb-serial-number> status [--index N]
//!   flash_repeater --port <usb-serial-number> abort [--index N]
//! ```
//!
//! The port is named by USB serial number, not `/dev/cu.*` path, for the same reason
//! `flash_portals` does: several serial functions are attached at once and the node
//! numbers move.

use std::io::Write;
use std::time::{Duration, Instant};

use router_link::repeater_ota::{self as ota, RepeaterImage, RepeaterOtaParams};
use router_link::rs485::{device::SerialPortDevice, Rs485};
use router_proto::repeater::{parse_reply, RepeaterTarget, RepeaterVerb};
use router_proto::Value;

const USAGE: &str = "\
usage:
  flash_repeater --port <usb-serial-number> <firmware.bin> [--index N] [--chunk 512]
                                                           [--gap 2] [--no-boot] [--dry-run]
  flash_repeater --port <usb-serial-number> status [--index N]
  flash_repeater --port <usb-serial-number> abort  [--index N]";

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
}

/// One reply to the verb just sent, or nothing if the repeater stayed quiet.
///
/// Matching on the verb matters here: a step that timed out earlier can have its answer
/// still in flight, and accepting it as this step's would report success for work that
/// never happened.
fn await_reply(
    rs485: &mut Rs485,
    verb: RepeaterVerb,
    timeout: Duration,
) -> Option<router_proto::repeater::RepeaterReply> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        for envelope in rs485.update() {
            if let Ok(Some(reply)) = parse_reply(&envelope.body) {
                if reply.verb == Some(verb) {
                    return Some(reply);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    None
}

/// Waits until a queued burst is actually on the wire.
///
/// An empty outbox is not enough. `SerialPortDevice::transmit` is a buffered `write_all`,
/// so the worker hands a whole streaming pass to the OS in a couple of seconds while the
/// port is still shifting it out for another half a minute. Asking for the map at that
/// point queues the request behind the backlog and times out against a repeater that is
/// doing exactly what it should.
fn drain(rs485: &mut Rs485, started: Instant, wire: Duration, timeout: Duration) -> bool {
    let deadline = started + timeout;
    while Instant::now() < deadline {
        rs485.update();
        if rs485.outbox_len() == 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    if rs485.outbox_len() != 0 {
        return false;
    }
    // The outbox emptying only means the OS took the bytes. Hold until the wire itself
    // could have carried them, measured from when the burst was queued.
    let until = started + wire + Duration::from_secs(2);
    while Instant::now() < until {
        rs485.update();
        std::thread::sleep(Duration::from_millis(20));
    }
    true
}

/// How long a streaming pass really takes.
///
/// The two costs overlap rather than add: the worker's per-packet sleep happens while the
/// OS is still shifting out earlier packets, so a pass takes the longer of "bytes at
/// 115200" and "one gap per packet", not their sum. Adding them overestimates by nearly
/// half, and overestimating is not harmless -- the receiver abandons a session after 30
/// seconds with no accepted chunk, so waiting too long before asking for the map loses
/// a transfer that had in fact completed.
///
/// 10 bits per byte, and 48 bytes of envelope, verb, bin header, CRC and COBS per chunk.
fn wire_time(count: usize, params: &RepeaterOtaParams) -> Duration {
    let bytes = count * (params.chunk_bytes + 48);
    let on_the_wire = bytes as f32 * 10.0 / 115_200.0;
    let paced = count as f32 * params.wait_between_chunks_ms as f32 / 1000.0;
    Duration::from_secs_f32(on_the_wire.max(paced) * 1.1)
}

fn field<'a>(payload: &'a Option<Value>, name: &str) -> Option<&'a Value> {
    let Some(Value::Map(entries)) = payload else {
        return None;
    };
    entries
        .iter()
        .find(|(k, _)| k.as_str() == Some(name))
        .map(|(_, v)| v)
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
    let index: u8 = args.value("--index").unwrap_or("1").parse()?;
    let target = ota::validate_index(index)?;

    match first.as_str() {
        "status" => {
            let mut rs485 = open_bus(&serial_number)?;
            rs485.transmit(router_link::rs485::Packet {
                payload: router_link::rs485::Payload::Rendered(
                    router_proto::repeater::request(&target, RepeaterVerb::Status, None),
                ),
                target: target.reply_source().unwrap_or(router_proto::HOST),
                address: String::new(),
                needs_ack: false,
                collateable: false,
                custom_wait_time_ms: Some(0),
                on_sent: None,
            });
            match await_reply(&mut rs485, RepeaterVerb::Status, Duration::from_secs(3)) {
                Some(reply) => println!("{reply:#?}"),
                None => println!("repeater {index}: no answer"),
            }
            rs485.close();
            Ok(())
        }
        "abort" => {
            let mut rs485 = open_bus(&serial_number)?;
            ota::abort(&rs485, &target);
            drain(&mut rs485, Instant::now(), Duration::ZERO, Duration::from_secs(3));
            println!("repeater {index}: abort sent");
            rs485.close();
            Ok(())
        }
        "probe" => probe(&args, &serial_number, index, target),
        path => flash(path, &args, &serial_number, index, target),
    }
}

/// Opens a session, sends exactly `--first N` chunks of a synthetic image, and reads the
/// map back. Separates "the repeater rejects the frame" from "the frames are not arriving":
/// at one or two chunks there is no throughput to blame.
fn probe(
    args: &Args,
    serial_number: &str,
    index: u8,
    target: RepeaterTarget,
) -> Result<(), Box<dyn std::error::Error>> {
    let count: usize = args.value("--first").unwrap_or("1").parse()?;
    let params = RepeaterOtaParams {
        chunk_bytes: args.value("--chunk").unwrap_or("512").parse()?,
        wait_between_chunks_ms: args.value("--gap").unwrap_or("50").parse()?,
        ..RepeaterOtaParams::default()
    };
    // Synthetic, but the same shape as a real image: enough chunks that indices are
    // meaningful, and a byte pattern that includes zeros so COBS has real work to do.
    let bytes: Vec<u8> = (0..params.chunk_bytes * count.max(8)).map(|i| (i % 251) as u8).collect();
    let image = RepeaterImage::new(bytes, params.chunk_bytes)?;

    let mut rs485 = open_bus(serial_number)?;
    ota::begin(&rs485, &target, &image, &params);
    let Some(reply) = await_reply(
        &mut rs485,
        RepeaterVerb::OtaBegin,
        Duration::from_millis(params.begin_timeout_ms as u64),
    ) else {
        return Err(format!("repeater {index} did not answer ota-begin").into());
    };
    println!("begin    ok={} payload={:?}", reply.ok, reply.payload);

    let indices: Vec<usize> = (0..count.min(image.chunk_count())).collect();
    let started = Instant::now();
    let queued = ota::send_chunks(&rs485, &target, &image, &params, &indices);
    let wire = wire_time(queued, &params);
    drain(&mut rs485, started, wire, wire * 3 + Duration::from_secs(30));
    println!("stream   {queued} chunk(s) of {} bytes sent", params.chunk_bytes);

    ota::request_map(&rs485, &target, &params);
    match await_reply(&mut rs485, RepeaterVerb::OtaMap, Duration::from_secs(6)) {
        Some(reply) => println!("map      {:?}", reply.payload),
        None => println!("map      no answer"),
    }
    if !args.present("--keep") {
        ota::abort(&rs485, &target);
        drain(&mut rs485, Instant::now(), Duration::ZERO, Duration::from_secs(3));
    }
    rs485.close();
    Ok(())
}

fn flash(
    path: &str,
    args: &Args,
    serial_number: &str,
    index: u8,
    target: RepeaterTarget,
) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = std::fs::read(path)?;
    let params = RepeaterOtaParams {
        chunk_bytes: args.value("--chunk").unwrap_or("512").parse()?,
        wait_between_chunks_ms: args.value("--gap").unwrap_or("2").parse()?,
        ..RepeaterOtaParams::default()
    };
    let image = RepeaterImage::new(bytes, params.chunk_bytes)?;

    println!("image    {path}  ({} bytes)", image.len());
    println!(
        "chunks   {} x {} bytes,  SHA-256 {}",
        image.chunk_count(),
        params.chunk_bytes,
        image
            .sha256()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    println!(
        "estimate {:.0}s of wire time per repeater\n",
        image.estimated_seconds(&params)
    );
    if args.present("--dry-run") {
        return Ok(());
    }

    let mut rs485 = open_bus(serial_number)?;
    let started = Instant::now();

    // ---- begin -------------------------------------------------------------------
    print!("begin    erasing the target slot... ");
    std::io::stdout().flush().ok();
    ota::begin(&rs485, &target, &image, &params);
    let Some(reply) = await_reply(
        &mut rs485,
        RepeaterVerb::OtaBegin,
        Duration::from_millis(params.begin_timeout_ms as u64),
    ) else {
        return Err(format!("repeater {index} did not answer ota-begin").into());
    };
    if !reply.ok {
        return Err(format!("repeater {index} refused ota-begin: {:?}", reply.payload).into());
    }
    println!("ok ({:.1}s)", started.elapsed().as_secs_f32());

    // ---- stream, then repair exactly the gaps -------------------------------------
    let mut indices = ota::all_indices(&image);
    let mut round = 0;
    loop {
        let label = if round == 0 {
            "stream  ".to_string()
        } else {
            format!("repair {round}")
        };
        print!(" {label} {} chunks... ", indices.len());
        std::io::stdout().flush().ok();
        let pass_started = Instant::now();
        let queued = ota::send_chunks(&rs485, &target, &image, &params, &indices);
        let wire = wire_time(queued, &params);
        if !drain(&mut rs485, pass_started, wire, wire * 3 + Duration::from_secs(30)) {
            return Err("the outbox did not drain; is the port still there?".into());
        }
        println!("{queued} sent ({:.0}s elapsed)", started.elapsed().as_secs_f32());

        ota::request_map(&rs485, &target, &params);
        let Some(reply) = await_reply(&mut rs485, RepeaterVerb::OtaMap, Duration::from_secs(15))
        else {
            return Err(format!("repeater {index} did not answer ota-map").into());
        };
        let Some(Value::Binary(bitmap)) = field(&reply.payload, "map") else {
            return Err(format!("ota-map reply had no bitmap: {:?}", reply.payload).into());
        };
        let missing = ota::missing_from_bitmap(bitmap, image.chunk_count());
        // The repeater's own count is printed alongside the bitmap's: when the two
        // disagree the fault is in the bitmap, not in the transfer.
        let claimed = field(&reply.payload, "got")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        println!(
            " map      {}/{} chunks present (repeater says {claimed}; bitmap {} bytes, {} set)",
            image.chunk_count() - missing.len(),
            image.chunk_count(),
            bitmap.len(),
            bitmap.iter().map(|b| b.count_ones()).sum::<u32>(),
        );
        if missing.is_empty() {
            break;
        }
        round += 1;
        if round > 5 {
            return Err(format!("{} chunks still missing after 5 repair rounds", missing.len()).into());
        }
        indices = missing;
    }

    // ---- commit -------------------------------------------------------------------
    print!("end      hashing the written slot... ");
    std::io::stdout().flush().ok();
    ota::end(&rs485, &target, &params);
    let Some(reply) = await_reply(
        &mut rs485,
        RepeaterVerb::OtaEnd,
        Duration::from_millis(params.end_timeout_ms as u64),
    ) else {
        return Err(format!("repeater {index} did not answer ota-end").into());
    };
    if !reply.ok {
        return Err(format!("repeater {index} refused ota-end: {:?}", reply.payload).into());
    }
    println!("ok ({:.0}s)", started.elapsed().as_secs_f32());

    if args.present("--no-boot") {
        println!("\ncommitted but not booted (--no-boot); send ota-boot when ready");
        rs485.close();
        return Ok(());
    }

    // ---- boot, then confirm --------------------------------------------------------
    ota::boot(&rs485, &target);
    drain(&mut rs485, Instant::now(), Duration::ZERO, Duration::from_secs(3));
    println!("boot     rebooting into the new slot...");
    // The repeater is not on the bus at all while it restarts, so this wait is real.
    std::thread::sleep(Duration::from_secs(5));
    rs485.update();

    print!("confirm  marking the new image good... ");
    std::io::stdout().flush().ok();
    ota::confirm(&rs485, &target, &params);
    match await_reply(&mut rs485, RepeaterVerb::OtaConfirm, Duration::from_secs(4)) {
        Some(reply) if reply.ok => println!("ok"),
        Some(reply) => println!("refused: {:?}", reply.payload),
        // Not fatal: an unconfirmed image resolves itself on local evidence in about
        // 30 seconds. Worth saying out loud, though, because until then a power cut
        // rolls the repeater back.
        None => println!("no answer -- it will self-confirm in ~30s"),
    }

    println!("\ndone in {:.0}s", started.elapsed().as_secs_f32());
    rs485.close();
    Ok(())
}
