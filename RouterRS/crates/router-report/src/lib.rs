//! NDJSON session reporting for RouterRS: a non-blocking `Reporter` handle
//! fans events from any thread into a single writer thread that owns the
//! session file, all aggregation state, per-portal health scoring, the live
//! diagnostics snapshot for the GUI, and the summary JSON.

pub mod events;
pub mod health;
pub mod reporter;
pub mod snapshot;
pub mod summary;
pub mod time;
pub(crate) mod writer;

pub use events::{Event, LatencyStats, Totals};
pub use reporter::{ReportConfig, Reporter, ReporterHandle, RxKind, SessionInfo};
pub use snapshot::{
    ColumnDiag, ColumnState, DiagnosticsSnapshot, FaultLine, PortalDiag, PortalState,
};
