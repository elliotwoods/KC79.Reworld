//! The portal test bench.
//!
//! One module on a bench, and everything you want to do to it: flash it, connect to it, drive
//! it, watch it, and come away with evidence. A human operates the page; an agent drives the
//! same bench over HTTP at the same time, through the same command queue, so the two can never
//! disagree about what is happening.
//!
//! ```text
//! portal-test-bench                 # native window + http://127.0.0.1:8770
//! portal-test-bench --headless      # the same page, no window
//! portal-test-bench --simulate      # a modelled module: no probe, no port, no board
//! portal-test-bench --port 9000     # insist on a port; occupied is then a hard error
//! ```
//!
//! `--port` is framework-owned and means *insist*. Without it the bound port is a preference:
//! a bench with the GUI up and a `ptb` session running is the normal case here, and refusing to
//! start because 8770 was taken would be the wrong answer. See [`preferred_or_free_port`].

use std::net::TcpListener;
use std::sync::Arc;

use av_gui_bus::{LiveBus, SchemaBuilder};
use av_gui_host::HostConfig;
use av_operator_app::{AppBuilder, AppResult, OperatorApp, RunContext, UiKind, WindowSpec};

mod bench_api;
mod flash;
mod schema;
mod worker;

/// Where plans live: `<repo>/PortalTestBench/plans`.
///
/// Resolved from the compiled-in manifest directory rather than the working directory, so the
/// binary finds its plans whether it was started from a shell, the launcher, or a debugger.
pub fn plans_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plans")
}

/// Where session files are written.
pub fn reports_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reports")
}

/// Preferred loopback port. Chosen to sit clear of the framework's own defaults (8730 host,
/// 8740 gallery) and of PortalFlasher's 8761, so a bench and a flasher can be open together.
const HTTP_PORT: u16 = 8770;

struct PortalTestBenchApp {
    simulate: bool,
    port: u16,
}

impl OperatorApp for PortalTestBenchApp {
    const NAME: &'static str = "portal-test-bench";

    /// Nothing is drawn *underneath* this page. The live plots are canvas in the DOM, and there
    /// is no camera, no 3D and no video -- so the compositor, the CEF payload and the helper
    /// subprocess would all be cost with nothing bought. Measured idle difference on this
    /// framework: roughly 8% of one core and 396 MB against 19-30% and 524 MB.
    const UI: UiKind = UiKind::ControlWindow;

    fn display_name() -> &'static str {
        "Portal Test Bench"
    }

    fn create(context: &RunContext) -> AppResult<Self> {
        Ok(Self {
            simulate: context.flag("--simulate"),
            port: HTTP_PORT,
        })
    }

    fn configure(&mut self, context: &RunContext, app: &mut AppBuilder) -> AppResult<()> {
        app.assets(av_operator_app::web_assets!("../../web/dist", "index.html"));

        let port = match context.pinned_port()? {
            Some(pinned) => pinned,
            None => {
                let (port, moved) = preferred_or_free_port();
                if moved {
                    // Say so rather than letting the operator find the page missing at the
                    // address they expected. `ptb` discovers the real port from the port file.
                    eprintln!(
                        "  port {HTTP_PORT} is in use, so this bench is on {port}. \
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
        schema::declare(builder, self.simulate).map_err(std::io::Error::other)?;
        Ok(())
    }

    fn start(
        &mut self,
        _context: &RunContext,
        live: &Arc<LiveBus>,
        app: &mut AppBuilder,
    ) -> AppResult<()> {
        let bus = live.current();
        let params = schema::Params::resolve(&bus).map_err(std::io::Error::other)?;
        schema::publish_setup(&bus, &params, self.port, self.simulate)
            .map_err(std::io::Error::other)?;

        // The session file. A bench that cannot write its evidence is still a usable
        // instrument, so a failure here is a warning rather than a refusal to start.
        let report =
            match bench_core::report::Report::open(&reports_dir(), env!("CARGO_PKG_VERSION")) {
                Ok(report) => report,
                Err(error) => {
                    eprintln!("  no session file: {error}");
                    bench_core::report::Report::disabled()
                }
            };

        let shared = Arc::new(worker::Shared::default());
        app.routes(bench_api::routes(Arc::clone(&shared)));

        let worker = worker::Worker::new(
            bus,
            params,
            bench_core::bench::Bench::new(report),
            shared,
            plans_dir(),
            self.simulate,
        );
        std::thread::Builder::new()
            .name("portal-test-bench-worker".into())
            .spawn(move || worker.run())?;
        Ok(())
    }

    fn window(
        &mut self,
        _context: &RunContext,
        _live: &Arc<LiveBus>,
    ) -> AppResult<Option<WindowSpec>> {
        Ok(Some(WindowSpec::new("Portal Test Bench", 1280, 900)))
    }
}

/// Bind [`HTTP_PORT`] if it is free, otherwise take an ephemeral one.
///
/// Returns the port and whether it moved. Racy by construction -- the listener is dropped
/// before the host binds -- but the host's own bind is the authority and fails loudly if it
/// loses the race, so this only ever costs a retry, never a wrong answer.
fn preferred_or_free_port() -> (u16, bool) {
    if let Ok(listener) = TcpListener::bind(("127.0.0.1", HTTP_PORT)) {
        drop(listener);
        return (HTTP_PORT, false);
    }
    match TcpListener::bind(("127.0.0.1", 0)).and_then(|l| l.local_addr()) {
        Ok(addr) => (addr.port(), true),
        // Nothing is bindable; let the host produce the real diagnostic.
        Err(_) => (HTTP_PORT, false),
    }
}

fn main() {
    PortalTestBenchApp::run();
}
