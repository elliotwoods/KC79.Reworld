//! Ask every id on one RS485 bus to identify itself, and print what comes back.
//!
//! This is the diagnostic that has to work before any firmware-update run means anything:
//! `flash_portals status` only asks the bootloader question, so a board running its
//! application answers it with silence -- which is indistinguishable from a board that is
//! not there at all, or from a bus that is not passing traffic.
//!
//! Here every reply is printed as received, whatever it is, so the three cases separate.
//!
//! ```text
//!   bus_probe --port <usb-serial-number> [--ids 1-9] [--verb p|poll|ping] [--wait-ms 400]
//! ```

use std::time::{Duration, Instant};

use router_link::rs485::{device::SerialPortDevice, Packet, Rs485};
use router_proto::commands;
use router_proto::envelope::Trailer;

const USAGE: &str = "\
usage:
  bus_probe --port <usb-serial-number> [--ids 1-9] [--verb p|poll|ping] [--wait-ms 400]";

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
                ids.extend(from..=to);
            }
            None => ids.push(
                part.trim()
                    .parse()
                    .map_err(|_| format!("bad id '{part}'"))?,
            ),
        }
    }
    Ok(ids)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut serial_number = None;
    let mut ids_spec = "1-9".to_string();
    let mut verb = "p".to_string();
    let mut wait_ms = 400u64;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--port" => serial_number = args.next(),
            "--ids" => ids_spec = args.next().unwrap_or_default(),
            "--verb" => verb = args.next().unwrap_or_default(),
            "--wait-ms" => wait_ms = args.next().unwrap_or_default().parse()?,
            other => return Err(format!("unknown argument '{other}'\n\n{USAGE}").into()),
        }
    }
    let serial_number = serial_number.ok_or(USAGE)?;
    let ids = parse_ids(&ids_spec)?;

    let body = match verb.as_str() {
        "p" => commands::poll_position(),
        "poll" => commands::poll(),
        "ping" => commands::ping(),
        other => return Err(format!("unknown verb '{other}'").into()),
    };

    let port = find_port(&serial_number)
        .ok_or_else(|| format!("no USB serial device with serial number {serial_number}"))?;
    println!("port     {port}  (serial {serial_number})");
    println!("verb     {verb}\n");

    let mut rs485 = Rs485::new(0, router_report::Reporter::disabled());
    rs485.open_device(Box::new(SerialPortDevice::open(&port)?));
    rs485.update();

    for id in ids {
        // `needs_ack` off and no post-send wait: the worker must not spend its own
        // timeout budget here, because we do the waiting and want to see every frame
        // that arrives, not only one that it accepts as an ACK.
        rs485.transmit(Packet {
            needs_ack: false,
            collateable: false,
            custom_wait_time_ms: Some(0),
            address: String::new(),
            ..Packet::from_body(id, &body, "")
        });

        let sent_at = Instant::now();
        let deadline = sent_at + Duration::from_millis(wait_ms);
        let mut heard = Vec::new();
        while Instant::now() < deadline {
            for envelope in rs485.update() {
                heard.push((sent_at.elapsed(), envelope));
            }
            std::thread::sleep(Duration::from_millis(5));
        }

        if heard.is_empty() {
            println!("{id:>4}  silent");
        }
        for (elapsed, envelope) in heard {
            let trailer = match envelope.trailer {
                Trailer::Absent => "no trailer".to_string(),
                Trailer::Ok { seq } => format!("trailer ok seq {seq}"),
                Trailer::Bad { expected, found } => {
                    format!("TRAILER BAD expected {expected:04X} found {found:04X}")
                }
            };
            println!(
                "{id:>4}  +{:>5.0}ms  from {:<4} [{trailer}]  {:?}",
                elapsed.as_secs_f32() * 1000.0,
                envelope.source,
                envelope.body
            );
        }
    }

    let stats = rs485.stats();
    println!(
        "\nsent {} frames, received {}, decode errors {}",
        stats.tx_count, stats.rx_count, stats.decode_errors
    );
    rs485.close();
    Ok(())
}
