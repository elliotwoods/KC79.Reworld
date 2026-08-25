//! `portal-flasher` — the production SWD rig for KC79 Portal boards.
//!
//! One binary that opens a native window **and** serves the identical page on loopback. The
//! operator normally watches neither: they seat a board, hear a tone, cycle it, hear a second
//! tone, and move on. The screen is for when something goes wrong.
//!
//! Everything the rig *decides* lives in `portal-swd`, which has no dependency on this stack at
//! all and is tested without a probe, a board, or a browser. What is left here is a schema, a
//! worker that turns bus values into machine inputs, and a page.
//!
//! # Running it
//!
//! ```text
//! portal-flasher                 # native window + http://127.0.0.1:8761
//! portal-flasher --headless      # the same page, no window -- and no tones (see below)
//! portal-flasher --simulate      # a modelled STM32G070, with a fixture switch on the page
//! portal-flasher --port 9000     # insist on a port; occupied is then a hard error
//! ```
//!
//! 8761 is a preference, not a requirement: if it is taken — nearly always by another
//! `portal-flasher` — a free port is used instead and both the reason and the address are printed.
//! A second window is a reasonable thing to want, and it should not need a command-line flag.
//!
//! **Sound lives in the browser**, because that is where the framework's system sounds are. A
//! headless rig is therefore a silent rig, which for a tool whose primary output is a tone is
//! not a mode anyone should flash boards in. The worker disarms itself when no session is
//! connected or the page's heartbeat goes stale, so this is enforced rather than documented.

mod device_api;
mod schema;
mod worker;

use std::sync::Arc;

use av_gui_bus::{LiveBus, SchemaBuilder};
use av_gui_host::HostConfig;
use av_operator_app::{AppBuilder, AppResult, OperatorApp, RunContext, WindowSpec};
use portal_swd::{ImageBundle, OptionBytePolicy, Region, RegionName, RunCheckSpec, SimRig};

use worker::Worker;

use std::sync::Mutex;

/// Preferred rather than fixed: an operator bookmarks it, and a fixture PC runs one of these — so
/// it should be the same number every time it can be, and never a reason not to start.
const HTTP_PORT: u16 = 8761;

/// The port to bind, preferring [`HTTP_PORT`] and falling back to whatever is free.
///
/// The framework resolves an explicit `--port` and deliberately makes an occupied one a hard
/// error: naming a port is an instruction, and quietly binding a different one would be ignoring
/// it. A *default* is not an instruction, and the same treatment turned "an instance is already
/// running" into
///
/// ```text
/// Only one usage of each socket address ... is normally permitted. (os error 10048)
/// ```
///
/// which names neither the port nor the program holding it. The second window is also the common
/// case here rather than a mistake — a flashing station and a page open to read a device are a
/// reasonable pair.
///
/// The probe-then-bind is a race in principle. In practice the loser gets exactly today's error,
/// so it can only improve on the current behaviour, and a lock file to close it would be more
/// machinery than a bench tool warrants.
fn preferred_or_free_port() -> (u16, bool) {
    match std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, HTTP_PORT)) {
        Ok(listener) => {
            drop(listener);
            (HTTP_PORT, false)
        }
        // Port 0 asks the OS for any free port. Asking it to choose is more reliable than
        // scanning upward from 8762, which races the same way and can march through a range
        // someone else is using.
        Err(_) => match std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)) {
            Ok(listener) => {
                let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
                drop(listener);
                (port, true)
            }
            // Nothing on the loopback interface can be bound at all, which is not a port
            // collision. Hand back the preferred port and let the host report the real reason.
            Err(_) => (HTTP_PORT, false),
        },
    }
}

struct PortalFlasherApp {
    simulate: bool,
    /// The port actually bound, which is not always [`HTTP_PORT`]. Recorded in `configure` so the
    /// banner in `start` cannot claim an address nothing is listening on.
    port: u16,
}

impl OperatorApp for PortalFlasherApp {
    const NAME: &'static str = "portal-flasher";

    fn display_name() -> &'static str {
        "Portal Flasher"
    }

    fn create(context: &RunContext) -> AppResult<Self> {
        Ok(Self {
            simulate: context.flag("--simulate"),
            port: HTTP_PORT,
        })
    }

    fn configure(&mut self, context: &RunContext, app: &mut AppBuilder) -> AppResult<()> {
        app.assets(av_operator_app::web_assets!("../../web/dist", "index.html"));

        // An explicit `--port` is left entirely alone: the framework overrides the bind after this
        // returns, and an occupied one stays a hard error there, which is right. Someone who names
        // a port means that port. It is still recorded, so the banner reports what was asked for
        // rather than the default it never used.
        let port = if let Some(pinned) = context.pinned_port()? {
            pinned
        } else {
            let (port, moved) = preferred_or_free_port();
            if moved {
                // Said before the host starts, because the line the host prints afterwards is the
                // *answer* and this is the reason for it. Almost always a second instance, and
                // that is worth naming rather than leaving as an inference about a port number.
                eprintln!(
                    "portal-flasher: {HTTP_PORT} is busy — most likely another portal-flasher is \
                     already running there. Using {port} instead; pass --port to insist on one."
                );
            }
            port
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

        // The device report is structured state, not a control surface, so it travels over a
        // namespaced route rather than the bus. See `device_api`.
        let device: device_api::DeviceState = Arc::new(Mutex::new(None));
        app.routes(device_api::routes(Arc::clone(&device)));

        let bundle = self.simulate.then(synthetic_bundle);
        let (rig, fixture): (Box<dyn portal_swd::Rig>, _) = if self.simulate {
            let sim = SimRig::new();
            let fixture = sim.fixture();
            (Box::new(sim), Some(fixture))
        } else {
            // No probe selector yet: a single-station rig takes the first probe it finds. The
            // page writes an explicit one once it has enumerated, which is what makes a second
            // ST-Link on the bench unambiguous rather than a coin toss.
            (Box::new(portal_swd::ProbeRsRig::new(None)), None)
        };

        println!(
            "{}: {} parameters, http://127.0.0.1:{}{}",
            Self::NAME,
            bus.schema().params().len(),
            self.port,
            if self.simulate { " (simulated)" } else { "" }
        );

        // A plain OS thread rather than a service: it blocks for seconds inside a flash pass, and
        // it must keep timing the removal gate whether or not anything is being rendered.
        let worker = Worker::new(bus, params, rig, bundle, device, self.simulate, fixture);
        std::thread::Builder::new()
            .name("portal-flasher-rig".into())
            .spawn(move || worker.run())
            .map_err(std::io::Error::other)?;

        Ok(())
    }

    fn window(
        &mut self,
        _context: &RunContext,
        _live: &Arc<LiveBus>,
    ) -> AppResult<Option<WindowSpec>> {
        Ok(Some(WindowSpec::new("Portal Flasher", 1200, 860)))
    }
}

/// An image that is obviously not a real one, for driving the rhythm without hardware.
///
/// Marked [`Provenance::Synthetic`] so the page says so: a simulated run that looked like a real
/// one in the log would be worse than no simulation at all.
pub(crate) fn synthetic_bundle() -> ImageBundle {
    use portal_swd::addr;

    // A v6 pair: an application linked for 0x08004000, carrying the descriptor that says so, and a
    // bootloader small enough to sit below it. The two halves have to agree here as strictly as
    // they do for a real image, because `ImageBundle::validate` is what the simulated pass runs
    // through and a bundle it refuses would make the whole rhythm untestable.
    let mut application = vec![0u8; 60_000];
    application[0..4].copy_from_slice(&0x2000_9000u32.to_le_bytes());
    application[4..8].copy_from_slice(&(addr::APP_BASE + 0x241).to_le_bytes());
    let at = addr::APP_DESCRIPTOR_OFFSET;
    application[at..at + 8].copy_from_slice(addr::APP_DESCRIPTOR_MAGIC);
    application[at + 8..at + 12].copy_from_slice(&addr::APP_BASE.to_le_bytes());

    ImageBundle {
        bootloader: Region::new(
            RegionName::Bootloader,
            portal_swd::addr::FLASH_BASE,
            vec![0xA5; 14_000],
        ),
        application: Region::new(
            RegionName::Application,
            portal_swd::addr::APP_BASE,
            application,
        ),
        option_bytes: OptionBytePolicy::default(),
        run_check: RunCheckSpec {
            liveness_address: 0x2000_0010,
            liveness_symbol: "g_liveness_counter".into(),
            ..RunCheckSpec::default()
        },
        provenance: portal_swd::image::Provenance::Synthetic,
        unselected: portal_swd::image::Unselected::Erase,
    }
}

fn main() {
    PortalFlasherApp::run();
}
