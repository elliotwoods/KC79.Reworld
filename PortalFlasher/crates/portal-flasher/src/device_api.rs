//! `GET /api/flasher/device` — what the last read found, in the shape the map draws.
//!
//! This is deliberately not on the bus. A parameter is a value an operator reads or sets; a
//! layout enum, two banners, a decoded option word and 256 occupancy buckets are structured
//! state about hardware the application owns. The framework's own router admin page draws the
//! same line for its log and traffic ring.
//!
//! The page polls this at human rate after a read completes — well inside `constraints.md` §5.

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use portal_swd::device::{DeviceImage, DeviceReport, RegionReport};
use portal_swd::{ImageBundle, addr};
use serde::Serialize;

/// How finely the map is bucketed. 256 over 128 kB is 512 bytes a bucket — finer than any
/// realistic pixel width, and small enough to send on every poll without thinking about it.
pub const BUCKETS: usize = 256;

#[derive(Clone, Debug, Serialize)]
pub struct RegionJson {
    pub name: String,
    pub base: String,
    pub used_bytes: usize,
    pub sha256: String,
    pub banner: Option<String>,
    /// Whether a plausible vector table sits at the head of this region.
    pub vector_ok: bool,
    pub entry: Option<String>,
}

impl RegionJson {
    fn of(region: &RegionReport) -> Self {
        Self {
            name: region.name.to_owned(),
            base: format!("{:#010X}", region.base),
            used_bytes: region.used_bytes,
            sha256: region.sha256.clone(),
            banner: region.banner.clone(),
            vector_ok: region.vector.is_some(),
            entry: region.vector.map(|v| format!("{:#010X}", v.entry())),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct OptionsJson {
    pub raw: String,
    pub rdp_level: u8,
    pub iwdg_sw: bool,
    pub nboot_sel: bool,
    pub nboot0: bool,
    pub nboot1: bool,
    pub nrst_mode: u8,
    pub warnings: Vec<String>,
}

/// Everything the device panel and the map need, in one document.
#[derive(Clone, Debug, Serialize)]
pub struct DeviceJson {
    pub read_at_ms: u64,
    pub layout: String,
    /// True when the layout is one the RS485 field updater can reach.
    pub field_updatable: bool,
    pub uid: String,
    pub idcode: Option<String>,
    pub dev_id: Option<String>,
    pub flash_kb: u16,
    pub programmed_bytes: usize,
    pub total_bytes: usize,
    pub regions: Vec<RegionJson>,
    /// Set when the whole flash is one image rather than two regions.
    pub flat_entry: Option<String>,
    pub options: OptionsJson,
    /// Programmed fraction per bucket, 0..=255, across `0x08000000..0x08020000`.
    pub occupancy: Vec<u8>,
    /// The same, for the image currently selected — the second lane of the map. `None` when
    /// nothing is selected, so the page draws one lane rather than a misleading empty one.
    pub selected_occupancy: Option<Vec<u8>>,
    /// Where the bank boundary falls, 0..1, so the map does not re-derive the memory map.
    pub split_fraction: f32,
}

impl DeviceJson {
    pub fn build(
        image: &DeviceImage,
        report: &DeviceReport,
        selected: Option<&ImageBundle>,
        read_at_ms: u64,
    ) -> Self {
        let options = report.options;
        Self {
            read_at_ms,
            layout: report.layout.as_str().to_owned(),
            field_updatable: report.layout.supports_field_update(),
            uid: report.uid.clone(),
            idcode: report.idcode.map(|v| format!("{v:#010X}")),
            dev_id: report.dev_id.map(|v| format!("{v:#05X}")),
            flash_kb: report.flash_kb,
            programmed_bytes: report.programmed_bytes,
            total_bytes: report.total_bytes,
            regions: vec![
                RegionJson::of(&report.bootloader),
                RegionJson::of(&report.application),
            ],
            flat_entry: report.flat_vector.map(|v| format!("{:#010X}", v.entry())),
            options: OptionsJson {
                raw: format!("{:#010X}", options.raw),
                rdp_level: options.rdp_level(),
                iwdg_sw: options.iwdg_sw,
                nboot_sel: options.nboot_sel,
                nboot0: options.nboot0,
                nboot1: options.nboot1,
                nrst_mode: options.nrst_mode,
                warnings: options.warnings().iter().map(|w| w.to_string()).collect(),
            },
            occupancy: image.occupancy(BUCKETS),
            selected_occupancy: selected
                .map(|bundle| portal_swd::device::occupancy_of(&bundle.expected_flash_image(), BUCKETS)),
            split_fraction: (addr::BOOTLOADER_BYTES as f32)
                / ((addr::FLASH_END - addr::FLASH_BASE) as f32),
        }
    }
}

/// The worker publishes here; the route reads. One `Mutex` around an `Option`, because a read is
/// a rare, whole-document replacement rather than a stream.
pub type DeviceState = Arc<Mutex<Option<DeviceJson>>>;

pub fn routes(state: DeviceState) -> Router {
    // Namespaced, per the application contract: this feature owns `/api/flasher/*` and does not
    // take a generic `/api`.
    Router::new()
        .route("/api/flasher/device", get(device))
        .with_state(state)
}

async fn device(State(state): State<DeviceState>) -> Result<axum::Json<DeviceJson>, StatusCode> {
    let guard = state.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    // 204 rather than an empty object: "nothing has been read yet" is a different thing from "a
    // read found nothing", and the page draws them differently.
    guard.clone().map(axum::Json).ok_or(StatusCode::NO_CONTENT)
}
