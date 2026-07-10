//! GUI-free core of the RouterRS app: domain model (Installation / Column /
//! Portal / Pilot), configuration, RS485 transport, servers, simulator,
//! image pipeline, and runtime.

pub mod config;
pub mod fw_update;
pub mod image;
pub mod model;
pub mod rs485;
pub mod runtime;
pub mod servers;
pub mod sim;

pub use glam::Vec2;
pub use router_proto as proto;
pub use router_report as report;
