//! GUI-free core of the RouterRS app: domain model (Installation / Column /
//! Portal / Pilot), configuration, servers, image pipeline, and runtime.

pub mod config;
pub mod image;
pub mod model;
pub mod runtime;
pub mod servers;

/// The RS485 link, re-exported from [`router_link`].
///
/// These three modules were moved out of this crate so that a program which only needs to talk
/// to a portal -- the test bench, a diagnostic tool -- does not also have to build the image
/// pipeline and the servers. They are re-exported at their original paths so that every
/// existing `router_core::rs485::…` / `::sim::…` / `::fw_update::…` reference keeps working;
/// nothing in RouterRS had to change.
pub use router_link::{fw_update, repeater_ota, rs485, sim};

pub use glam::Vec2;
pub use router_link as link;

/// Repeaters per shared outer bus in the V3 topology.
pub use router_proto::REPEATER_COUNT;
pub use router_proto as proto;
pub use router_report as report;
