//! `ptb` -- the command-line front door to the portal test bench.
//!
//! This is the agent's tool. Everything the GUI can do to a module, `ptb` can do from a shell,
//! against the *same* running bench, so a human can watch a plot move while an agent drives.
//!
//! ```text
//! ptb state                       what is on the bench right now, as JSON
//! ptb run <plan> --wait           run it, block, print the verdict; exit code IS the verdict
//! ptb do home --axis a            one operation, executed as a one-step plan
//! ptb watch <run-id>              tail a detached run to its verdict
//! ptb abort [run-id]
//! ```
//!
//! # Why this is a separate binary rather than a `--script` flag
//!
//! The operator app cannot exit cleanly from a batch run. In a windowless run the framework's
//! lifecycle treats a background service that *returns successfully* as
//! `"background service {name} stopped unexpectedly"`, and the only clean stop is a shutdown
//! notified by the launcher's control socket, which application code cannot reach. A `--script`
//! flag could therefore only ever exit non-zero or bypass shutdown with `process::exit`. A
//! separate binary running the same `bench-core` engine is the honest answer, and it gets real
//! exit codes.
//!
//! # Exit codes
//!
//! The exit code is the answer, so `ptb run ... && deploy` means what it looks like.

use std::process::ExitCode;

/// The verdict, as an exit code.
///
/// Declared as a complete set from the start, and documented in `USAGE`, because the exit code
/// is part of this tool's contract with whatever is scripting it. The variants not yet
/// returned by any command are reached as the engine lands.
#[allow(dead_code, reason = "the full contract is declared up front; see USAGE")]
mod exit {
    /// Every criterion held.
    pub const PASS: u8 = 0;
    /// The run completed and a criterion did not hold. A real answer, not an error.
    pub const FAIL: u8 = 1;
    /// Stopped before it could decide -- operator abort, or a limit.
    pub const ABORTED: u8 = 2;
    /// The bench could not carry out the test: link lost, wrong firmware, bad plan.
    pub const ERROR: u8 = 3;
    /// No bench to talk to, and `--local` was not requested.
    pub const NO_BENCH: u8 = 4;
}

const USAGE: &str = "\
ptb -- drive the portal test bench from a shell

USAGE:
    ptb [--bench <url>] [--local] [--sim] <command> [args]

COMMANDS:
    ports                       list serial ports and debug probes on this machine
    listen <port> [--as <kind>] read a port and print what the module says, as JSON lines
    state                       print the whole bench state as JSON
    version                     print the bench and protocol versions

GLOBAL OPTIONS:
    --bench <url>   talk to a bench at this address (default: discover it)
    --local         own the hardware in this process instead of talking to a GUI
    --sim           with --local, use a modelled module: no probe, no port, no board

EXIT CODES:
    0 pass    1 fail    2 aborted    3 error    4 no bench found
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    match run(&args) {
        Ok(code) => ExitCode::from(code),
        Err(message) => {
            eprintln!("ptb: {message}");
            ExitCode::from(exit::ERROR)
        }
    }
}

fn run(args: &[String]) -> Result<u8, String> {
    let command = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .ok_or_else(|| "no command given; try `ptb --help`".to_string())?;

    match command.as_str() {
        "version" => {
            println!(
                "{{\"ptb\":\"{}\",\"report_profile\":\"{}\"}}",
                env!("CARGO_PKG_VERSION"),
                bench_core::REPORT_PROFILE
            );
            Ok(exit::PASS)
        }
        "ports" => {
            println!("{}", serde_json::to_string_pretty(&bench_core::survey()).map_err(|e| e.to_string())?);
            Ok(exit::PASS)
        }
        "listen" => listen(args),
        // Wired to /api/bench/state in M1, once there is a link to report on. Until then it
        // says so plainly rather than printing an empty document that looks like an answer.
        "state" => Err("`ptb state` needs the bench worker, which lands with the transports".into()),
        other => Err(format!("unknown command `{other}`; try `ptb --help`")),
    }
}

/// Open a port, read it, and print every event as a JSON line.
///
/// The point of printing NDJSON rather than prose is that this is the shape an agent can
/// filter: `ptb listen COM7 | jq -c 'select(.event=="log")'`. It is also the fastest way to
/// answer "is there anything alive on this port, and what is it".
fn listen(args: &[String]) -> Result<u8, String> {
    use bench_core::transport::LinkKind;

    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let port = positional
        .get(1)
        .ok_or_else(|| "usage: ptb listen <port> [--as vcp|bench-ascii|rs485-serial]".to_string())?;

    let kind = match option(args, "--as").unwrap_or("bench-ascii") {
        "vcp" => LinkKind::Vcp,
        "bench-ascii" => LinkKind::BenchAscii,
        "rs485-serial" => LinkKind::Rs485Serial,
        other => return Err(format!("unknown link kind `{other}`")),
    };
    let seconds: u64 = option(args, "--for").and_then(|v| v.parse().ok()).unwrap_or(5);

    let mut link = bench_core::open_link(kind, port)?;
    link.open().map_err(|e| e.to_string())?;

    // Ask who is there rather than only waiting: the production firmware says nothing at all
    // until spoken to, so a silent port would otherwise be indistinguishable from a dead one.
    if let Err(error) = link.send(&bench_core::transport::Op::Identify) {
        eprintln!("ptb: identify not sent: {error}");
    }

    let start = std::time::Instant::now();
    let mut seen = 0usize;
    while start.elapsed().as_secs() < seconds {
        for event in link.poll(start.elapsed().as_millis() as u64) {
            seen += 1;
            println!("{}", bench_core::event_json(&event));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    link.close();

    if seen == 0 {
        // An empty result is a finding, and must not read as a successful test.
        eprintln!("ptb: nothing was received from {port} in {seconds}s");
        return Ok(exit::FAIL);
    }
    Ok(exit::PASS)
}

fn option<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    let index = args.iter().position(|a| a == flag)?;
    args.get(index + 1).map(|s| s.as_str())
}
