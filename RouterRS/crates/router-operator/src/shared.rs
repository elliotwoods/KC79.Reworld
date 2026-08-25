//! Mirrors and the agent request queue, shared between the bridge thread and the HTTP
//! handlers. Handlers never touch the runtime: they read these mirrors or append a request
//! and return; the bridge drains requests into the one command queue on its next tick.

use std::sync::Mutex;
use std::sync::Arc;

use router_core::runtime::{Command, UiSnapshot};
use router_report::DiagnosticsSnapshot;

#[derive(Default)]
pub struct Shared {
    /// The latest model snapshot (swapped whole; readers clone the Arc).
    pub snapshot: Mutex<Arc<UiSnapshot>>,
    /// The latest diagnostics snapshot (~1 Hz).
    pub diag: Mutex<Arc<DiagnosticsSnapshot>>,
    /// Cached serial-port enumeration (~1 Hz; enumeration can block, handlers must not).
    pub ports: Mutex<Vec<String>>,
    /// Commands queued by `/api/router/*` handlers, drained by the bridge in arrival order.
    pub requests: Mutex<Vec<Command>>,
}

impl Shared {
    pub fn queue(&self, command: Command) {
        self.requests.lock().unwrap().push(command);
    }
}
