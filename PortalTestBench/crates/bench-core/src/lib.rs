//! Portal bench behaviour.
//!
//! Everything the bench *does* lives here: how it talks to a module, how a test plan is
//! described and executed, how a verdict is decided, and how evidence is recorded. The
//! operator app on top is a schema, a worker and a page; the CLI is an argument parser.
//!
//! This crate has no `av-*` dependency and no hardware dependency it cannot simulate, which
//! is what makes the whole suite runnable in CI and `ptb --local` possible at all.

pub mod bench;
pub mod dut;
pub mod engine;
pub mod plan;
pub mod report;
pub mod state;
pub mod survey;
pub mod threshold;
pub mod transport;
pub mod verdict;

pub use survey::{PortEntry, ProbeEntry, Survey, event_json, open_link, survey};

/// The report profile this build writes.
///
/// An additive profile of RouterRS's `docs/report-schema.md` v1: same envelope, same writer,
/// same file naming, with bench-specific event types layered on. Recorded in every session so
/// a reader years later can tell what it is holding.
pub const REPORT_PROFILE: &str = "bench/1";

/// Firmware log levels, from `PortalFW/src/Logger.h`.
///
/// Not an enum: they arrive off the wire as integers and unknown values must survive rather
/// than being coerced into the nearest variant.
pub const LOG_LEVEL_STATUS: u8 = 0;
pub const LOG_LEVEL_WARNING: u8 = 10;
pub const LOG_LEVEL_ERROR: u8 = 20;
