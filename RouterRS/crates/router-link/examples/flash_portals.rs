//! Update Portal firmware over one RS485 bus, from the command line.
//!
//! Drives [`router_link::fw_session`] rather than re-implementing anything: which of the
//! two protocols a board gets, whether the image belongs in the bank it is aimed at, and
//! the announce timing that keeps a legacy bootloader resident are all decided there and
//! covered by tests there.
//!
//! ```text
//!   flash_portals --port <usb-serial-number> app <image.bin> [--ids 1-54 | --serials ...]
//!                                                           [--mode auto|legacy|v6] [--no-run]
//!   flash_portals --port <usb-serial-number> bootloader <bl.bin> --id N [--stay]
//!   flash_portals --port <usb-serial-number> status [--ids 1-54]
//! ```
//!
//! The port is named by **USB serial number**, never by `/dev/cu.*` path: the node number
//! changes across reconnects, and this repo has three different serial functions that can
//! be attached at once.
//!
//! `status` is read-only. It does not recall anything, so a board running its application
//! answers as one -- which is itself the answer to "is this board in its bootloader".

use std::io::Write;
use std::time::{Duration, Instant};

use router_link::bootloader_update::{BlPhase, BlUpdateParams, BootloaderUpdate};
use router_link::fw_session::{
    Board, BoardKind, BoardState, FwSession, FwSessionParams, Mode, Phase, Targets,
};
use router_link::fw_update::{self, FwUpdateParams};
use router_link::rs485::{device::SerialPortDevice, Packet, Payload, Rs485};
use router_proto::bootloader::{self, BlReply, BlSelector};
use router_proto::replies::{classify_reply, Reply};
use router_proto::Value;

const USAGE: &str = "\
usage:
  flash_portals --port <usb-serial-number> app <image.bin> [--ids 1-54] [--serials 73001,73002]
                                                           [--mode auto|legacy|v6|blind] [--no-run]
                                                           [--legacy-gap ms] [--legacy-reps n]
                                                           [--passes n] [--chunk 64] [--gap ms]
  flash_portals --port <usb-serial-number> bootloader <bl.bin> --id N [--stay]
  flash_portals --port <usb-serial-number> status [--ids 1-54]
  flash_portals --port <usb-serial-number> bl <verb> --id N [--recall] [--serial S | --uid HEX]
                                           [--len L --crc C --chunk K --base B] [--to N]
                                           [--repeat n] [--timeout ms]";

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

/// `1-54`, `1,3,5`, `1-4,9` -- whichever an operator reaches for.
fn parse_ids(spec: &str) -> Result<Vec<i8>, String> {
    let mut ids = Vec::new();
    for part in spec.split(',').filter(|part| !part.is_empty()) {
        match part.split_once('-') {
            Some((from, to)) => {
                let from: i8 = from
                    .trim()
                    .parse()
                    .map_err(|_| format!("bad id '{from}'"))?;
                let to: i8 = to.trim().parse().map_err(|_| format!("bad id '{to}'"))?;
                if to < from {
                    return Err(format!("'{part}' counts backwards"));
                }
                ids.extend(from..=to);
            }
            None => ids.push(
                part.trim()
                    .parse()
                    .map_err(|_| format!("bad id '{part}'"))?,
            ),
        }
    }
    if ids.is_empty() {
        return Err("no ids".into());
    }
    Ok(ids)
}

fn parse_serials(spec: &str) -> Result<Vec<u32>, String> {
    spec.split(',')
        .filter(|part| !part.is_empty())
        .map(|part| {
            part.trim()
                .parse::<u32>()
                .map_err(|_| format!("bad serial '{part}'"))
        })
        .collect()
}

/// A flat `--flag value` / `--flag` scan. Deliberately not a dependency: the whole
/// surface is three subcommands and six flags.
struct Args {
    positional: Vec<String>,
    flags: std::collections::HashMap<String, Option<String>>,
}

impl Args {
    fn parse() -> Self {
        const VALUED: [&str; 18] = [
            "--port",
            "--ids",
            "--serials",
            "--mode",
            "--id",
            "--legacy-gap",
            "--legacy-reps",
            "--passes",
            "--chunk",
            "--gap",
            "--serial",
            "--uid",
            "--to",
            "--len",
            "--crc",
            "--base",
            "--repeat",
            "--timeout",
        ];
        let mut positional = Vec::new();
        let mut flags = std::collections::HashMap::new();
        let mut args = std::env::args().skip(1).peekable();
        while let Some(arg) = args.next() {
            if let Some(name) = arg.strip_prefix("--") {
                let name = format!("--{name}");
                let value = if VALUED.contains(&name.as_str()) {
                    args.next()
                } else {
                    None
                };
                flags.insert(name, value);
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

fn open_bus(serial_number: &str) -> Result<Rs485, Box<dyn std::error::Error>> {
    let port = find_port(serial_number)
        .ok_or_else(|| format!("no USB serial device with serial number {serial_number}"))?;
    println!("port     {port}  (serial {serial_number})");
    let mut rs485 = Rs485::new(0, router_report::Reporter::disabled());
    rs485.open_device(Box::new(SerialPortDevice::open(&port)?));
    rs485.update();
    if !rs485.is_connected() {
        // The worker sets this on its first successful poll; give it a moment.
        std::thread::sleep(Duration::from_millis(200));
        rs485.update();
    }
    Ok(rs485)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let Some(command) = args.positional.first().cloned() else {
        println!("{USAGE}");
        return Ok(());
    };
    let serial_number = args
        .value("--port")
        .ok_or("--port <usb-serial-number> is required")?
        .to_string();

    match command.as_str() {
        "app" => flash_application(&args, &serial_number),
        "bootloader" => flash_bootloader(&args, &serial_number),
        "status" => report_status(&args, &serial_number),
        "bl" => run_verb(&args, &serial_number),
        other => Err(format!("unknown subcommand '{other}'\n\n{USAGE}").into()),
    }
}

// ------------------------------------------------------------------ application

fn flash_application(args: &Args, serial_number: &str) -> Result<(), Box<dyn std::error::Error>> {
    let path = args
        .positional
        .get(1)
        .ok_or_else(|| format!("app needs an image path\n\n{USAGE}"))?;
    let firmware = std::fs::read(path)?;

    let targets = match (args.value("--ids"), args.value("--serials")) {
        (Some(_), Some(_)) => return Err("give --ids or --serials, not both".into()),
        (_, Some(serials)) => Targets::Serials(parse_serials(serials)?),
        (Some(ids), None) => Targets::Ids(parse_ids(ids)?),
        (None, None) => Targets::Ids(parse_ids("1-54")?),
    };
    let mode = match args.value("--mode").unwrap_or("auto") {
        "auto" => Mode::Auto,
        "legacy" => Mode::LegacyOnly,
        "v6" => Mode::V6Only,
        "blind" => Mode::Blind,
        other => return Err(format!("unknown mode '{other}'").into()),
    };

    let mut params = FwSessionParams {
        targets,
        mode,
        run_after: !args.present("--no-run"),
        ..FwSessionParams::default()
    };
    // The blind path's pacing is the only control there is over a v4/v5 bootloader: its
    // receive ring overflows if frames arrive faster than it writes them ("RB Full" in its
    // own log), and it refuses every frame after a gap ("Write position is ahead of ours"),
    // so one overrun ends the upload. Repetitions do not help -- a repeated frame is sent
    // immediately after the original, which is when the receiver is least able to take it.
    if let Some(gap) = args.value("--legacy-gap") {
        params.legacy.wait_between_frames_ms = gap.parse()?;
    }
    if let Some(reps) = args.value("--legacy-reps") {
        params.legacy.frame_repetitions = reps.parse()?;
    }
    if let Some(passes) = args.value("--passes") {
        params.stream_passes = passes.parse()?;
    }
    // Frame size is the lever that matters on the V3 bus: a 128-byte chunk makes a ~176-byte
    // frame, and frames much over 160 bytes do not survive the relay intact. 64 halves that.
    if let Some(chunk) = args.value("--chunk") {
        params.chunk_bytes = chunk.parse()?;
    }
    if let Some(gap) = args.value("--gap") {
        params.data_gap_ms = gap.parse()?;
    }
    if let Targets::Ids(ids) = &params.targets {
        if ids.len() > 4 && !args.present("--legacy-gap") && !args.present("--legacy-reps") {
            params = FwSessionParams {
                mode: params.mode,
                run_after: params.run_after,
                ..FwSessionParams::mass(ids.clone())
            };
        }
    }

    let mut session = FwSession::new(&firmware, params)?;
    let (base, source) = session.image_base();
    println!("image    {path}  ({} bytes)", firmware.len());
    println!(
        "linked   0x{base:08X} ({}), padded to {} bytes, CRC-32C {:08X}",
        match source {
            router_proto::app_image::BaseSource::Descriptor => "from its descriptor",
            router_proto::app_image::BaseSource::InferredLegacy => "inferred from the reset vector",
        },
        session.image_len(),
        session.image_crc32()
    );
    println!("chunks   {}\n", session.chunk_count());
    // Anything knowable without the bus is reported before the port is opened, so an
    // operator who reached for the wrong build hears about it immediately.
    session.preflight()?;

    let mut rs485 = open_bus(serial_number)?;
    let started = Instant::now();
    let mut last_phase = Phase::Validate;
    let mut last_line = Instant::now();
    let progress = loop {
        let envelopes = rs485.update();
        let progress = session.tick(&rs485, Instant::now(), &envelopes);
        if progress.phase != last_phase {
            println!(
                "\r{:>6.0}s  {:<12} {}                    ",
                started.elapsed().as_secs_f32(),
                phase_name(progress.phase),
                progress.detail
            );
            last_phase = progress.phase;
        } else if last_line.elapsed() >= Duration::from_secs(2) {
            print!(
                "\r{:>6.0}s  {:<12} {:>5.1}%  {}/{} packets   ",
                started.elapsed().as_secs_f32(),
                phase_name(progress.phase),
                100.0 * progress.fraction,
                progress.packets_sent,
                progress.packets_queued
            );
            std::io::stdout().flush().ok();
            last_line = Instant::now();
        }
        if progress.done {
            break progress;
        }
        std::thread::sleep(Duration::from_millis(5));
    };

    println!("\n{}", board_table(&progress.boards));
    let stats = rs485.stats();
    println!(
        "sent {} frames, received {}, decode errors {}, in {:.0}s",
        stats.tx_count,
        stats.rx_count,
        stats.decode_errors,
        started.elapsed().as_secs_f32()
    );
    println!(
        "{}: {}",
        if progress.ok { "OK" } else { "FAILED" },
        progress.detail
    );
    rs485.close();
    if progress.ok {
        Ok(())
    } else {
        Err(progress.detail.into())
    }
}

fn phase_name(phase: Phase) -> &'static str {
    match phase {
        Phase::Validate => "validate",
        Phase::Bump => "recall",
        Phase::Discover => "discover",
        Phase::LegacyUpload => "broadcast",
        Phase::Begin => "erase",
        Phase::Stream => "stream",
        Phase::Map => "map",
        Phase::Repair { .. } => "repair",
        Phase::Verify => "verify",
        Phase::Run => "run",
        Phase::Done => "done",
    }
}

fn board_table(boards: &[Board]) -> String {
    let mut out =
        String::from("  id  kind        state                     base        application\n");
    for board in boards {
        let kind = match board.kind {
            BoardKind::Unknown => "unknown",
            BoardKind::V6 => "v6",
            BoardKind::Legacy => "legacy",
            BoardKind::AppRunning => "app running",
            BoardKind::Absent => "absent",
        };
        let state = match &board.state {
            BoardState::Pending => "pending".to_string(),
            BoardState::Began => "session open".to_string(),
            BoardState::Streamed => "streamed".to_string(),
            BoardState::Missing(chunks) => format!("missing {} chunks", chunks.len()),
            BoardState::Complete => "complete".to_string(),
            BoardState::Verified { crc32 } => format!("verified {crc32:08X}"),
            BoardState::VerifyFailed => "VERIFY FAILED".to_string(),
            BoardState::Running => "running".to_string(),
            BoardState::NoReply(phase) => format!("no reply in {}", phase_name(*phase)),
            BoardState::Refused(why) => format!("refused: {why}"),
            BoardState::LegacyBlind => "sent blind".to_string(),
        };
        let base = if board.base == 0 {
            "-".to_string()
        } else {
            format!("0x{:08X}", board.base)
        };
        let app = board
            .app
            .as_ref()
            .map(|app| app.version.clone())
            .unwrap_or_default();
        out.push_str(&format!(
            "  {:>2}  {kind:<11} {state:<25} {base:<11} {app}\n",
            board.id
        ));
    }
    out
}

// ------------------------------------------------------------------- bootloader

fn flash_bootloader(args: &Args, serial_number: &str) -> Result<(), Box<dyn std::error::Error>> {
    let path = args
        .positional
        .get(1)
        .ok_or_else(|| format!("bootloader needs an image path\n\n{USAGE}"))?;
    let id: i8 = args
        .value("--id")
        .ok_or("bootloader needs --id N")?
        .parse()?;
    let image = std::fs::read(path)?;

    let params = BlUpdateParams {
        id,
        stay: args.present("--stay"),
        ..BlUpdateParams::default()
    };
    let mut update = BootloaderUpdate::new(&image, params)?;
    println!("image    {path}  ({} bytes)", image.len());
    println!(
        "padded   {} bytes, CRC-32C {:08X}, to portal {id}\n",
        update.image_len(),
        update.image_crc32()
    );
    println!(
        "WARNING  commit leaves this board with no valid bootloader for about half a second.\n\
         \x20        Losing power inside that window needs an ST-Link on the SWD header to fix.\n"
    );

    let mut rs485 = open_bus(serial_number)?;
    let started = Instant::now();
    let mut last_phase = BlPhase::Escape;
    let progress = loop {
        let envelopes = rs485.update();
        let progress = update.tick(&rs485, Instant::now(), &envelopes);
        if progress.phase != last_phase {
            println!(
                "{:>6.0}s  {}",
                started.elapsed().as_secs_f32(),
                progress.detail
            );
            last_phase = progress.phase;
        } else if matches!(progress.phase, BlPhase::Data { .. }) {
            print!("\r        chunk {}/{}   ", progress.chunk, progress.chunks);
            std::io::stdout().flush().ok();
        }
        if progress.done {
            break progress;
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    println!(
        "\n{}: {}",
        if progress.ok { "OK" } else { "FAILED" },
        progress.detail
    );
    rs485.close();
    if progress.ok {
        Ok(())
    } else {
        Err(progress.detail.into())
    }
}

// ----------------------------------------------------------------------- status

/// Send one `bl` verb and print what comes back, decoded and measured.
///
/// The update paths drive whole sequences; this drives a single verb, which is what a verb
/// matrix needs and what nothing else could do. `adopt` and `reset` in particular had builders in
/// `router-proto` and no caller anywhere, so until now they were unreachable from any tool.
///
/// The raw reply length is printed alongside the decode because on a relayed bus it is the
/// diagnostic that matters: a reply can decode as far as it goes and still be missing its tail.
fn run_verb(args: &Args, serial_number: &str) -> Result<(), Box<dyn std::error::Error>> {
    let verb = args
        .positional
        .get(1)
        .ok_or_else(|| format!("bl needs a verb\n\n{USAGE}"))?
        .clone();
    let id: i8 = args.value("--id").unwrap_or("1").parse()?;
    let repeat: usize = args.value("--repeat").unwrap_or("1").parse()?;
    let timeout_ms: u64 = args.value("--timeout").unwrap_or("2000").parse()?;

    let selector = match (args.value("--serial"), args.value("--uid")) {
        (Some(_), Some(_)) => return Err("give --serial or --uid, not both".into()),
        (Some(serial), None) => BlSelector::Serial(serial.parse()?),
        (None, Some(hex)) => {
            let bytes: Vec<u8> = (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
                .collect::<Result<_, _>>()?;
            let uid: [u8; 12] = bytes
                .try_into()
                .map_err(|_| "--uid must be 24 hex characters (12 bytes)")?;
            BlSelector::Uid(uid)
        }
        (None, None) => BlSelector::None,
    };
    // A selector is how a board is named when the frame is not addressed to it, so a selected
    // request is broadcast. Unicast otherwise.
    let target = if matches!(selector, BlSelector::None) {
        id
    } else {
        -1
    };

    let mut rs485 = open_bus(serial_number)?;
    println!();

    // A board running its application ignores `bl` entirely -- deliberately, so that scanning for
    // bootloaders cannot be mistaken for finding broken applications. `--recall` bounces it into
    // its bootloader first, which is what makes a single verb testable against a healthy board.
    if args.present("--recall") {
        for step in fw_update::announce_steps(&FwUpdateParams::default()) {
            rs485.transmit(fw_update::step_packet(step, &[]));
        }
        // Drain the announce burst before asking anything. `announce_steps` queues five seconds
        // of alternating words, and a verb pushed in behind them is not transmitted until they
        // have all gone -- so it times out having never reached the bus, which reads as a board
        // that did not answer.
        let deadline = Instant::now() + Duration::from_secs(12);
        while Instant::now() < deadline {
            rs485.update();
            if rs485.outbox_len() == 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        // Then let the application finish its half-second pause and the bootloader come up.
        let settle = Instant::now() + Duration::from_millis(1_200);
        while Instant::now() < settle {
            rs485.update();
            std::thread::sleep(Duration::from_millis(10));
        }
        println!("  (recalled into the bootloader)");
    }

    for attempt in 0..repeat {
        let seq = ((attempt % 255) + 1) as u8;
        let bytes = match verb.as_str() {
            "status" => bootloader::status(target, selector.clone(), seq),
            "map" => bootloader::map_request(
                target,
                args.value("--chunk").map(|c| c.parse()).transpose()?,
                seq,
            ),
            "verify" => bootloader::verify(target, seq),
            "run" => bootloader::run(target, seq),
            "reset" => bootloader::reset(target, seq),
            "adopt" => bootloader::adopt(
                target,
                selector.clone(),
                args.value("--to")
                    .ok_or("adopt needs --to <new id>")?
                    .parse()?,
                seq,
            ),
            "begin" => bootloader::begin(
                target,
                args.value("--len").ok_or("begin needs --len")?.parse()?,
                u32::from_str_radix(
                    args.value("--crc")
                        .ok_or("begin needs --crc (hex)")?
                        .trim_start_matches("0x"),
                    16,
                )?,
                args.value("--chunk").unwrap_or("128").parse()?,
                args.value("--base")
                    .map(|b| u32::from_str_radix(b.trim_start_matches("0x"), 16))
                    .transpose()?,
                seq,
            ),
            // Anything the protocol does not define goes out verbatim, which is the only way to
            // test that a board answers `UNKNOWN_VERB` rather than sitting there silently. It has
            // to be built from the envelope encoder rather than `bootloader::request`, because
            // that takes a `BlVerb` and the whole point here is a name that is not one.
            other => {
                let body = Value::Map(vec![(
                    Value::from("bl"),
                    Value::Map(vec![(Value::from("q"), Value::from(other))]),
                )]);
                router_proto::envelope::encode_envelope_trailer(target, &body, seq)
            }
        };

        rs485.transmit(Packet {
            payload: Payload::Rendered(bytes),
            target,
            address: String::new(),
            needs_ack: false,
            collateable: false,
            custom_wait_time_ms: Some(2),
            on_sent: None,
        });

        let sent = Instant::now();
        let deadline = sent + Duration::from_millis(timeout_ms);
        let mut answered = false;
        while Instant::now() < deadline {
            for envelope in rs485.update() {
                let trailer = match envelope.trailer {
                    router_proto::envelope::Trailer::Absent => "no trailer".to_string(),
                    router_proto::envelope::Trailer::Ok { seq } => format!("trailer ok, seq {seq}"),
                    router_proto::envelope::Trailer::Bad { expected, found } => {
                        format!("TRAILER BAD expected {expected:04X} found {found:04X}")
                    }
                };
                println!(
                    "  {verb} -> {target}  reply from {} after {:.0} ms  [{trailer}]",
                    envelope.source,
                    sent.elapsed().as_secs_f32() * 1000.0
                );
                match classify_reply(&envelope.body) {
                    Reply::Bootloader(reply) => println!("    {reply:#?}"),
                    other => println!("    (not a bootloader reply) {other:?}"),
                }
                answered = true;
            }
            if answered {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        if !answered {
            println!("  {verb} -> {target}  silence for {timeout_ms} ms");
        }
    }

    rs485.close();
    Ok(())
}

/// Ask each board what it is, and change nothing.
///
/// No recall: a board running its application answers a `bl` request with an ordinary
/// ACK, and "this board is running its application" is exactly what an operator wants to
/// know before deciding to interrupt it.
fn report_status(args: &Args, serial_number: &str) -> Result<(), Box<dyn std::error::Error>> {
    let ids = parse_ids(args.value("--ids").unwrap_or("1-54"))?;
    let mut rs485 = open_bus(serial_number)?;
    println!();

    for (index, id) in ids.iter().enumerate() {
        let seq = (index % 256) as u8;
        rs485.transmit(Packet {
            payload: Payload::Rendered(bootloader::status(*id, BlSelector::None, seq)),
            target: *id,
            // The same two transport rules the update paths obey: the worker would
            // consume this reply as an ACK, and a shared address would collate.
            address: String::new(),
            needs_ack: false,
            collateable: false,
            custom_wait_time_ms: Some(2),
            on_sent: None,
        });

        let deadline = Instant::now() + Duration::from_millis(400);
        let mut row = format!("  {id:>2}  silent (a v4/v5 bootloader, or nothing there)");
        while Instant::now() < deadline {
            let mut answered = false;
            for envelope in rs485.update() {
                if envelope.source != *id || !envelope.trailer.acceptable() {
                    continue;
                }
                row = match classify_reply(&envelope.body) {
                    Reply::Bootloader(BlReply::Status(status)) => format!(
                        "  {id:>2}  bootloader v{}  base 0x{:08X}  serial {}  {}",
                        status.version,
                        status.base,
                        status
                            .serial
                            .map(|serial| serial.to_string())
                            .unwrap_or_else(|| "-".into()),
                        status
                            .app
                            .map(|app| app.version)
                            .unwrap_or_else(|| "no application".into()),
                    ),
                    Reply::Ack(_) | Reply::Report(_) => {
                        format!("  {id:>2}  running its application")
                    }
                    other => format!("  {id:>2}  unexpected reply {other:?}"),
                };
                answered = true;
            }
            if answered {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        println!("{row}");
    }

    rs485.close();
    Ok(())
}
