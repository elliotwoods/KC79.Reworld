//! The Router as an av-frameworks operator app.
//!
//! One installation, and everything the operator does to it: connect its columns, drive its
//! portals, render an image into it, update its firmware, and come away with a session report.
//! A human operates the page; external systems keep the C++-era OSC (:4000) and REST (:8080)
//! interfaces, and agents drive `/api/router/*` on the same host that serves the page.
//!
//! ```text
//! router                       # native window + http://127.0.0.1:8780
//! router --headless            # the same page, no window
//! router --simulate            # in-process simulated buses: no serial, no gateways
//! router --port 9000           # insist on a port; occupied is then a hard error
//! ```
//!
//! Domain flags are unchanged from the iced app: `--config <path>`, `--report-dir <dir>`,
//! `--verbose`, `--sim-dead/-noisy <ids>`, `--sim-drop/-corrupt <0..1>`.

use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;

use av_gui_bus::{LiveBus, SchemaBuilder};
use av_gui_host::HostConfig;
use av_operator_app::{
    AppBuilder, AppResult, OperatorApp, RunContext, RunOutcome, UiKind, WindowSpec,
};
use router_core::config::AppConfig;
use router_core::runtime::{RuntimeConfig, RuntimeHandle};
use router_core::sim::SimConfig;
use router_report::{ReportConfig, Reporter, ReporterHandle, SessionInfo};

mod api;
mod bridge;
mod schema;
mod shared;

/// Preferred loopback port. Chosen clear of the framework's own defaults (8730 host, 8740
/// gallery), PortalFlasher's 8761, PortalTestBench's 8770, and this app's own legacy servers
/// (REST 8080, OSC 4000), so all of them can be open together.
const HTTP_PORT: u16 = 8780;

/// Where `config.json` lives, resolved without depending on the working directory.
///
/// 1. `--config <path>` (handled in `create`, wins outright).
/// 2. `ROUTER_CONFIG`, for a run driving a config that is not the tree's.
/// 3. `<repo>/RouterRS/config.json` from the compiled-in manifest directory — the development
///    answer, and the same file the C++ Router-compatible tooling edits.
fn default_config_path() -> PathBuf {
    if let Some(explicit) = env_dir("ROUTER_CONFIG") {
        return explicit;
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config.json")
}

/// Where session reports are written. Same tier shape as [`default_config_path`]; the
/// development answer is `<repo>/RouterRS/reports`, which is what the RouterReports viewer
/// reads (`../RouterRS/reports`) — resolved from the manifest rather than the working
/// directory so the viewer finds sessions whether the router started from a shell, the
/// launcher, or a debugger.
fn default_report_dir() -> PathBuf {
    if let Some(explicit) = env_dir("ROUTER_REPORTS") {
        return explicit;
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../reports")
}

fn env_dir(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

struct RouterOperatorApp {
    config_path: PathBuf,
    app_config: Option<AppConfig>,
    simulate: Option<SimConfig>,
    report_dir: PathBuf,
    verbose: bool,
    port: u16,
    shape: schema::Shape,
    runtime: Option<RuntimeHandle>,
    reporter_handle: Option<ReporterHandle>,
    bridge_stop: Arc<std::sync::atomic::AtomicBool>,
    bridge_join: Option<std::thread::JoinHandle<()>>,
}

impl OperatorApp for RouterOperatorApp {
    const NAME: &'static str = "router";

    /// Nothing is drawn *underneath* this page — the installation grid, pilot disk, dials,
    /// heatmap and image preview are all canvas in the DOM — so the control window is the
    /// lightest kind that carries the product. See `av-app.toml`.
    const UI: UiKind = UiKind::ControlWindow;

    fn display_name() -> &'static str {
        "Router"
    }

    fn tracing_filter() -> &'static str {
        "info,av_gui_host=info"
    }

    fn create(context: &RunContext) -> AppResult<Self> {
        // Valued domain flags are parsed from the raw argument vector — the same spelling the
        // iced app accepted, so launch scripts survive the migration unchanged.
        let args: Vec<String> = std::env::args().skip(1).collect();
        let get_value = |flag: &str| -> Option<String> {
            args.iter()
                .position(|a| a == flag)
                .and_then(|i| args.get(i + 1))
                .cloned()
        };

        let config_path = get_value("--config")
            .map(PathBuf::from)
            .unwrap_or_else(default_config_path);
        let app_config = AppConfig::load(&config_path)
            .unwrap_or_else(|_| AppConfig::from_json(serde_json::json!({})));

        let simulate = if context.flag("--simulate") {
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
            Some(sim)
        } else {
            None
        };

        let report_dir = get_value("--report-dir")
            .map(PathBuf::from)
            .unwrap_or_else(default_report_dir);

        Ok(Self {
            config_path,
            app_config: Some(app_config),
            simulate,
            report_dir,
            verbose: context.flag("--verbose"),
            port: HTTP_PORT,
            shape: schema::Shape::default(),
            runtime: None,
            reporter_handle: None,
            bridge_stop: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            bridge_join: None,
        })
    }

    fn configure(&mut self, context: &RunContext, app: &mut AppBuilder) -> AppResult<()> {
        app.assets(av_operator_app::web_assets!("../../web/dist", "index.html"));

        let port = match context.pinned_port()? {
            Some(pinned) => pinned,
            None => {
                let (port, moved) = preferred_or_free_port();
                if moved {
                    eprintln!(
                        "  port {HTTP_PORT} is in use, so this router is on {port}. \
                         Pass --port {HTTP_PORT} to insist."
                    );
                }
                port
            }
        };
        self.port = port;
        app.host(HostConfig {
            bind: ([127, 0, 0, 1], port).into(),
            ..Default::default()
        });
        Ok(())
    }

    fn declare(&mut self, _context: &RunContext, builder: &mut SchemaBuilder) -> AppResult<()> {
        let config = self.app_config.as_ref().expect("config loaded in create");
        let shape = schema::Shape::from_config(config);
        schema::declare(builder, &shape, self.simulate.is_some()).map_err(std::io::Error::other)?;
        self.shape = shape;
        Ok(())
    }

    fn start(
        &mut self,
        _context: &RunContext,
        live: &Arc<LiveBus>,
        _app: &mut AppBuilder,
    ) -> AppResult<()> {
        let bus = live.current();
        let params =
            schema::Params::resolve(&bus, &self.shape).map_err(std::io::Error::other)?;
        schema::publish_setup(&bus, &params, self.port, self.simulate.is_some(), &self.config_path)
            .map_err(std::io::Error::other)?;

        let app_config = self.app_config.take().expect("config loaded in create");
        let session = SessionInfo {
            app_version: format!("router {}", env!("CARGO_PKG_VERSION")),
            host: hostname(),
            config: serde_json::json!({
                "columns": app_config.installation.arrangement.columns,
                "rows": app_config.installation.arrangement.rows,
                "simulate": self.simulate.is_some(),
            }),
        };
        let report_config = ReportConfig {
            dir: self.report_dir.clone(),
            verbose: self.verbose,
            ..Default::default()
        };
        // A router that cannot write its session evidence is still a usable instrument, so a
        // failure here is a hard error only because the runtime needs a Reporter; it is
        // surfaced as a startup error rather than a panic.
        let (reporter, reporter_handle) =
            Reporter::start(report_config, session).map_err(std::io::Error::other)?;
        self.reporter_handle = Some(reporter_handle);

        // Per-column device settings from the config, seeded into the device-picker params.
        let initial_devices: Vec<Option<serde_json::Value>> = (0..self.shape.columns.len())
            .map(|i| app_config.installation.columns.get(i).and_then(|c| c.rs485.clone()))
            .collect();

        let runtime = router_core::runtime::spawn(RuntimeConfig {
            app_config,
            config_path: Some(self.config_path.clone()),
            simulate: self.simulate.clone(),
            reporter: reporter.clone(),
        });

        let shared = Arc::new(shared::Shared::default());
        _app.routes(api::routes(shared.clone()));

        let bridge = bridge::Bridge::new(
            live.clone(),
            params,
            self.shape.clone(),
            runtime.command_sender(),
            runtime.snapshot_slot(),
            reporter,
            shared,
            self.bridge_stop.clone(),
            initial_devices,
        );
        self.bridge_join = Some(
            std::thread::Builder::new()
                .name("router-bridge".into())
                .spawn(move || bridge.run())?,
        );
        self.runtime = Some(runtime);
        Ok(())
    }

    fn window(
        &mut self,
        _context: &RunContext,
        _live: &Arc<LiveBus>,
    ) -> AppResult<Option<WindowSpec>> {
        let mut spec = WindowSpec::new("Router", 1600, 960);
        // The mark `av_app_icon::embed_for` resolved for this package at build time — the same
        // bytes the Windows resource carries — so the macOS Dock wears it too (a bare binary
        // has no bundle to carry `CFBundleIconFile`).
        spec.options.icon =
            av_operator_app::AppIcon::Ico(include_bytes!(env!("AV_APP_ICON_ICO")).as_slice().into());
        Ok(Some(spec))
    }

    fn shutdown(
        &mut self,
        _context: &RunContext,
        _live: &Arc<LiveBus>,
        _outcome: RunOutcome,
    ) -> AppResult<()> {
        // Order matters: the bridge feeds the runtime, so it stops first; the reporter is
        // shut down (writing its summary) only after the runtime has fully stopped.
        self.bridge_stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(join) = self.bridge_join.take() {
            let _ = join.join();
        }
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown();
        }
        if let Some(handle) = self.reporter_handle.take() {
            if let Some(summary) = handle.shutdown() {
                eprintln!("  session summary: {}", summary.display());
            }
        }
        Ok(())
    }
}

fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".into())
}

/// Bind [`HTTP_PORT`] if it is free, otherwise take an ephemeral one.
///
/// Returns the port and whether it moved. Racy by construction — the listener is dropped
/// before the host binds — but the host's own bind is the authority and fails loudly if it
/// loses the race, so this only ever costs a retry, never a wrong answer.
fn preferred_or_free_port() -> (u16, bool) {
    if let Ok(listener) = TcpListener::bind(("127.0.0.1", HTTP_PORT)) {
        drop(listener);
        return (HTTP_PORT, false);
    }
    match TcpListener::bind(("127.0.0.1", 0)).and_then(|l| l.local_addr()) {
        Ok(addr) => (addr.port(), true),
        Err(_) => (HTTP_PORT, false),
    }
}

fn main() {
    RouterOperatorApp::run();
}
