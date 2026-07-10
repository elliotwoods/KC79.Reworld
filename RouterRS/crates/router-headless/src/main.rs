//! Headless RouterRS runtime: loads config.json, runs the model thread with
//! REST + OSC servers (and optionally the firmware simulator), and reports
//! to NDJSON. Ctrl-C to exit cleanly (writes session summary).
//!
//! Usage:
//!   router-headless [--config <path>] [--simulate] [--report-dir <dir>]
//!                   [--verbose] [--duration <seconds>]
//!                   [--sim-dead <ids>] [--sim-noisy <ids>]
//!                   [--sim-drop <0..1>] [--sim-corrupt <0..1>]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use router_core::config::AppConfig;
use router_core::runtime::{self, RuntimeConfig};
use router_core::sim::SimConfig;
use router_report::{ReportConfig, Reporter, SessionInfo};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let get_value = |flag: &str| -> Option<String> {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let has_flag = |flag: &str| args.iter().any(|a| a == flag);

    if has_flag("--help") || has_flag("-h") {
        println!(
            "router-headless [--config <path>] [--simulate] [--report-dir <dir>]\n\
             \x20               [--verbose] [--duration <seconds>]\n\
             \x20               [--sim-dead <ids>] [--sim-noisy <ids>]\n\
             \x20               [--sim-drop <0..1>] [--sim-corrupt <0..1>]"
        );
        return;
    }

    let config_path = PathBuf::from(get_value("--config").unwrap_or_else(|| "config.json".into()));
    let app_config = match AppConfig::load(&config_path) {
        Ok(config) => {
            println!("loaded {}", config_path.display());
            config
        }
        Err(e) => {
            println!("no config loaded ({e}); using defaults");
            AppConfig::from_json(serde_json::json!({}))
        }
    };

    let simulate = if has_flag("--simulate") {
        let parse_ids = |flag: &str| -> Vec<u8> {
            get_value(flag)
                .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect())
                .unwrap_or_default()
        };
        let mut sim = SimConfig::default();
        sim.dead_portals = parse_ids("--sim-dead");
        sim.noisy_portals = parse_ids("--sim-noisy");
        if let Some(rate) = get_value("--sim-drop").and_then(|v| v.parse().ok()) {
            sim.drop_rate = rate;
        }
        if let Some(rate) = get_value("--sim-corrupt").and_then(|v| v.parse().ok()) {
            sim.corrupt_rate = rate;
        }
        println!(
            "simulating hardware (dead: {:?}, noisy: {:?}, drop: {}, corrupt: {})",
            sim.dead_portals, sim.noisy_portals, sim.drop_rate, sim.corrupt_rate
        );
        Some(sim)
    } else {
        None
    };

    // reporter
    let report_dir = PathBuf::from(get_value("--report-dir").unwrap_or_else(|| "reports".into()));
    let report_config = ReportConfig {
        dir: report_dir,
        verbose: has_flag("--verbose"),
        ..Default::default()
    };
    let session = SessionInfo {
        app_version: format!("router-headless {}", env!("CARGO_PKG_VERSION")),
        host: std::env::var("COMPUTERNAME")
            .or_else(|_| std::env::var("HOSTNAME"))
            .unwrap_or_else(|_| "unknown".into()),
        config: serde_json::json!({
            "columns": app_config.installation.arrangement.columns,
            "rows": app_config.installation.arrangement.rows,
            "columnWidth": app_config.installation.arrangement.column_width,
            "simulate": simulate.is_some(),
        }),
    };
    let (reporter, reporter_handle) =
        Reporter::start(report_config, session).expect("start reporter");

    let handle = runtime::spawn(RuntimeConfig {
        app_config,
        config_path: Some(config_path),
        simulate,
        reporter: reporter.clone(),
    });

    // wait for the model thread's first published snapshot (device connects
    // can block a few seconds when configured TCP gateways are unreachable)
    let wait_start = Instant::now();
    while handle.snapshot().generation == 0 && wait_start.elapsed() < Duration::from_secs(30) {
        std::thread::sleep(Duration::from_millis(50));
    }
    // --poll <seconds>: enable scheduled polling on every column (useful for
    // diagnostics sessions; per-portal status and ACK tracking need polls)
    if let Some(period_s) = get_value("--poll").and_then(|v| v.parse::<f32>().ok()) {
        let count = handle.snapshot().columns.len().max(64);
        for col in 0..count {
            handle.send(router_core::runtime::Command::SetScheduledPoll {
                col,
                enabled: true,
                period_s,
            });
        }
        println!("scheduled poll every {period_s}s on all columns");
    }

    let snapshot = handle.snapshot();
    println!(
        "runtime up: {} columns, resolution {}x{}, REST :{} ({}), OSC :{} ({})",
        snapshot.columns.len(),
        snapshot.resolution.0,
        snapshot.resolution.1,
        snapshot.rest_port,
        if snapshot.rest_running { "on" } else { "off" },
        snapshot.osc_port,
        if snapshot.osc_running { "on" } else { "off" },
    );

    // Ctrl-C -> clean shutdown
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        let _ = ctrlc::set_handler(move || stop.store(true, Ordering::Release));
    }

    let duration = get_value("--duration")
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_secs);
    let started = Instant::now();
    let mut last_status = Instant::now();

    while !stop.load(Ordering::Acquire) {
        if let Some(limit) = duration {
            if started.elapsed() >= limit {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(200));
        if last_status.elapsed() >= Duration::from_secs(10) {
            last_status = Instant::now();
            let snap = handle.snapshot();
            let (tx, rx, timeouts): (u64, u64, u64) = snap.columns.iter().fold((0, 0, 0), |acc, c| {
                (
                    acc.0 + c.stats.tx_count,
                    acc.1 + c.stats.rx_count,
                    acc.2 + c.stats.ack_timeouts,
                )
            });
            println!(
                "[{}s] tx {} rx {} timeouts {} connected {}/{}",
                started.elapsed().as_secs(),
                tx,
                rx,
                timeouts,
                snap.columns.iter().filter(|c| c.stats.connected).count(),
                snap.columns.len(),
            );
        }
    }

    println!("shutting down...");
    handle.shutdown();
    if let Some(summary) = reporter_handle.shutdown() {
        println!("session summary: {}", summary.display());
    }
}
