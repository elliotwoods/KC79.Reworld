//! Is this board's RS485 receive line electrically connected to anything?
//!
//! ```text
//! cargo run --example rs485_line_probe -- [seconds]
//! ```
//!
//! Every other diagnostic on this bench observes the bus from one end: the host adapter,
//! or the repeater's own counters. Both report silence identically whether the far end is
//! not transmitting or the pair is not landing on the connector, and that ambiguity has
//! cost more bench time than any protocol question. This looks at the pin instead.
//!
//! `PA3` is USART2_RX -- the Portal's RS485 receive (`PortalFW/pins.md`). Sampling
//! `GPIOA->IDR` while something drives the branch answers the question directly:
//!
//! * the bit toggles  -- the pair is connected and the far end is driving it. Any failure
//!   after this point is protocol or firmware, not wiring.
//! * stuck at 1       -- idle mark. Either nothing is driving, or the pair is not landing
//!   here and the line is simply biased idle.
//! * stuck at 0       -- held at space: A/B swapped, shorted, or a stuck driver. A UART
//!   reads this as a permanent break, never as bytes.
//!
//! It writes nothing and does not halt the core, so the application keeps running
//! throughout -- which matters, because a halted MCU cannot receive and would fake the
//! very silence being investigated.

#![cfg(feature = "probe")]

use std::time::{Duration, Instant};

use probe_rs::architecture::arm::{
    FullyQualifiedApAddress, dp::DpAddress, sequences::DefaultArmSequence,
};
use probe_rs::probe::list::Lister;

/// STM32G0 GPIO port A, and the input data register within it (RM0454 s6.4.5).
const GPIOA_BASE: u64 = 0x5000_0000;
const GPIOB_BASE: u64 = 0x5000_0400;
const GPIO_IDR: u64 = 0x10;
/// USART2_RX / RS485 receive, USART2_TX, and the RS485 driver-enable.
const PIN_RS485_RX: u32 = 3;
const PIN_RS485_TX: u32 = 2;
const PIN_RS485_DE: u32 = 1;

fn main() {
    if let Err(err) = run() {
        eprintln!("\nFAILED: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let seconds: f64 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(3.0);

    let lister = Lister::new();
    let probes = lister.list_all();
    let info = probes
        .first()
        .ok_or("no probes. Is the ST-Link plugged in?")?;
    let mut probe = info.open()?;
    println!(
        "probe   {} serial={:?}",
        probe.get_name(),
        info.serial_number.as_deref().unwrap_or("-")
    );

    // Non-invasive attach: no session, no halt, no reset. See `probe_spike`.
    probe.attach_to_unspecified()?;
    let mut iface = match probe.try_into_arm_debug_interface(DefaultArmSequence::create()) {
        Ok(iface) => iface,
        Err((_probe, err)) => return Err(format!("could not open the ARM interface: {err}").into()),
    };
    iface.select_debug_port(DpAddress::Default)?;
    let ap = FullyQualifiedApAddress::v1_with_default_dp(0);
    let mut mem = iface.memory_interface(&ap)?;

    println!("sampling GPIOA->IDR for {seconds:.1}s -- drive the branch now\n");

    let mut samples: u64 = 0;
    let mut transitions = [0u64; 4];
    let mut low_samples = [0u64; 4];
    let mut previous: Option<u32> = None;
    // Self-check. "The pin never moved" is only evidence if this sampler can see a pin
    // that does move, so port B is watched purely to find one -- the heartbeat LED and
    // the debug UART live there and something on a running board should be busy.
    let mut port_b_transitions = [0u64; 16];
    let mut previous_b: Option<u32> = None;
    let started = Instant::now();
    let window = Duration::from_secs_f64(seconds);

    while started.elapsed() < window {
        let idr = mem.read_word_32(GPIOA_BASE + GPIO_IDR)?;
        samples += 1;
        for (slot, pin) in [PIN_RS485_RX, PIN_RS485_TX, PIN_RS485_DE]
            .iter()
            .enumerate()
        {
            let bit = (idr >> pin) & 1;
            if bit == 0 {
                low_samples[slot] += 1;
            }
            if let Some(prev) = previous {
                if (prev >> pin) & 1 != bit {
                    transitions[slot] += 1;
                }
            }
        }
        previous = Some(idr);

        let idr_b = mem.read_word_32(GPIOB_BASE + GPIO_IDR)?;
        if let Some(prev) = previous_b {
            for (pin, slot) in port_b_transitions.iter_mut().enumerate() {
                if (prev >> pin) & 1 != (idr_b >> pin) & 1 {
                    *slot += 1;
                }
            }
        }
        previous_b = Some(idr_b);
    }

    let rate = samples as f64 / started.elapsed().as_secs_f64();
    println!(
        "{samples} samples in {:.2}s ({rate:.0} Hz)\n",
        started.elapsed().as_secs_f32()
    );
    println!(
        "{:<14} {:>10} {:>12} {:>9}",
        "pin", "edges", "low samples", "verdict"
    );
    for (slot, (name, pin)) in [
        ("PA3 RS485 RX", PIN_RS485_RX),
        ("PA2 RS485 TX", PIN_RS485_TX),
        ("PA1 RS485 DE", PIN_RS485_DE),
    ]
    .iter()
    .enumerate()
    {
        let edges = transitions[slot];
        let low = low_samples[slot];
        let verdict = if edges > 0 {
            "TOGGLING - connected and driven"
        } else if low == 0 {
            "stuck high (idle mark)"
        } else if low == samples {
            "stuck low (break / swapped pair)"
        } else {
            "indeterminate"
        };
        let _ = pin;
        println!("{name:<14} {edges:>10} {low:>12} {verdict:>9}");
    }

    let busy: Vec<String> = port_b_transitions
        .iter()
        .enumerate()
        .filter(|(_, count)| **count > 0)
        .map(|(pin, count)| format!("PB{pin}={count}"))
        .collect();
    println!(
        "\nself-check, port B edges: {}",
        if busy.is_empty() {
            "NONE -- the sampler saw no movement anywhere, so a quiet PA3 proves nothing".into()
        } else {
            format!("{} -- the sampler does observe live pins", busy.join(" "))
        }
    );

    println!(
        "\nNote: this samples over SWD at a few kHz, far below the 115200 baud bit rate, so\n\
         the edge count is a presence test, not a byte count. One edge is enough to prove\n\
         the pair is connected; zero edges across a burst is the finding."
    );

    drop(mem);
    let _probe = iface.close();
    Ok(())
}
