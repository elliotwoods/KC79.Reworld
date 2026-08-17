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
//! ```
//!
//! **Sound lives in the browser**, because that is where the framework's system sounds are. A
//! headless rig is therefore a silent rig, which for a tool whose primary output is a tone is
//! not a mode anyone should flash boards in. The worker disarms itself when no session is
//! connected or the page's heartbeat goes stale, so this is enforced rather than documented.

mod schema;
mod worker;

use std::sync::Arc;

use av_gui_bus::{LiveBus, SchemaBuilder};
use av_gui_host::HostConfig;
use av_operator_app::{AppBuilder, AppResult, OperatorApp, RunContext, WindowSpec};
use portal_swd::{ImageBundle, OptionBytePolicy, Region, RegionName, RunCheckSpec, SimRig};

use worker::{NoRig, Worker};

/// Fixed rather than ephemeral: an operator bookmarks it, and a fixture PC runs one of these.
const HTTP_PORT: u16 = 8761;

struct PortalFlasherApp {
    simulate: bool,
}

impl OperatorApp for PortalFlasherApp {
    const NAME: &'static str = "portal-flasher";

    fn display_name() -> &'static str {
        "Portal Flasher"
    }

    fn create(context: &RunContext) -> AppResult<Self> {
        Ok(Self {
            simulate: context.flag("--simulate"),
        })
    }

    fn configure(&mut self, _context: &RunContext, app: &mut AppBuilder) -> AppResult<()> {
        app.assets(av_operator_app::web_assets!("../../web/dist", "index.html"));
        app.host(HostConfig {
            bind: ([127, 0, 0, 1], HTTP_PORT).into(),
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
        _app: &mut AppBuilder,
    ) -> AppResult<()> {
        let bus = live.current();
        let params = schema::Params::resolve(&bus).map_err(std::io::Error::other)?;

        let bundle = self.simulate.then(synthetic_bundle);
        let (rig, fixture): (Box<dyn portal_swd::Rig>, _) = if self.simulate {
            let sim = SimRig::new();
            let fixture = sim.fixture();
            (Box::new(sim), Some(fixture))
        } else {
            (Box::new(NoRig), None)
        };

        println!(
            "{}: {} parameters, http://127.0.0.1:{HTTP_PORT}{}",
            Self::NAME,
            bus.schema().params().len(),
            if self.simulate { " (simulated)" } else { "" }
        );

        // A plain OS thread rather than a service: it blocks for seconds inside a flash pass, and
        // it must keep timing the removal gate whether or not anything is being rendered.
        let worker = Worker::new(bus, params, rig, bundle, fixture);
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
fn synthetic_bundle() -> ImageBundle {
    let mut application = vec![0u8; 60_000];
    application[0..4].copy_from_slice(&0x2000_9000u32.to_le_bytes());
    application[4..8].copy_from_slice(&(portal_swd::addr::APP_BASE + 0x241).to_le_bytes());

    ImageBundle {
        bootloader: Region::new(
            RegionName::Bootloader,
            portal_swd::addr::FLASH_BASE,
            vec![0xA5; 22_708],
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
    }
}

fn main() {
    PortalFlasherApp::run();
}
