//! The cloneable, non-blocking `Reporter` handle and its writer thread
//! lifecycle. `emit` never blocks: a full channel increments a drop counter
//! that the writer later reports as a `dropped_events` line.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::events::Event;
use crate::snapshot::DiagnosticsSnapshot;
use crate::writer;

#[derive(Debug, Clone)]
pub struct ReportConfig {
    /// Output directory for session files (created if missing).
    pub dir: PathBuf,
    /// Write raw packet_tx/packet_rx events.
    pub verbose: bool,
    /// bus_stats cadence (default 10 s).
    pub stats_interval: Duration,
    /// Summary checkpoint cadence (default 5 min).
    pub checkpoint_interval: Duration,
    pub channel_capacity: usize,
}

impl Default for ReportConfig {
    fn default() -> Self {
        Self {
            dir: PathBuf::from("reports"),
            verbose: false,
            stats_interval: Duration::from_secs(10),
            checkpoint_interval: Duration::from_secs(300),
            channel_capacity: 8192,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SessionInfo {
    pub app_version: String,
    pub host: String,
    /// Column topology etc., embedded in session_start.
    pub config: serde_json::Value,
}

/// Reply classification for rx accounting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RxKind {
    Ack,
    Report,
    Other,
}

impl RxKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RxKind::Ack => "ack",
            RxKind::Report => "report",
            RxKind::Other => "other",
        }
    }
}

/// Internal channel message.
pub(crate) enum Msg {
    Event(Event),
    /// A pre-rendered payload from a caller with its own event vocabulary.
    ///
    /// The writer still stamps `v`, `ts` and `seq`, so a downstream profile shares one file,
    /// one sequence and one storm-free path with the bus events rather than opening a second
    /// log that has to be correlated by timestamp afterwards.
    Line(serde_json::Value),
    /// Hot-path packet accounting (no allocation, no line written unless the
    /// event variant was chosen at emit time).
    Tx {
        col: u8,
        target: i8,
        needs_ack: bool,
        bytes: u32,
    },
    Rx {
        col: u8,
        source: i8,
        kind: RxKind,
        bytes: u32,
        latency_ms: Option<f32>,
    },
    OutboxDepth {
        col: u8,
        depth: u32,
    },
    WriteSummary(SyncSender<PathBuf>),
    Shutdown {
        reason: &'static str,
    },
}

pub(crate) struct Shared {
    pub dropped: AtomicU64,
    pub verbose: AtomicBool,
    pub snapshot: Mutex<Arc<DiagnosticsSnapshot>>,
}

/// Cloneable event-emitting handle. A `Reporter::disabled()` no-op variant
/// exists for tests and GUI-only runs.
#[derive(Clone)]
pub struct Reporter {
    tx: Option<SyncSender<Msg>>,
    shared: Arc<Shared>,
}

impl Reporter {
    pub fn start(
        config: ReportConfig,
        session: SessionInfo,
    ) -> std::io::Result<(Reporter, ReporterHandle)> {
        let (tx, rx): (SyncSender<Msg>, Receiver<Msg>) = sync_channel(config.channel_capacity);
        let shared = Arc::new(Shared {
            dropped: AtomicU64::new(0),
            verbose: AtomicBool::new(config.verbose),
            snapshot: Mutex::new(Arc::new(DiagnosticsSnapshot::default())),
        });

        std::fs::create_dir_all(&config.dir)?;
        let writer_shared = shared.clone();
        let join = std::thread::Builder::new()
            .name("report-writer".into())
            .spawn(move || writer::run(config, session, rx, writer_shared))?;

        let reporter = Reporter {
            tx: Some(tx.clone()),
            shared,
        };
        Ok((
            reporter,
            ReporterHandle {
                tx,
                join: Some(join),
            },
        ))
    }

    /// A reporter that discards everything.
    pub fn disabled() -> Reporter {
        Reporter {
            tx: None,
            shared: Arc::new(Shared {
                dropped: AtomicU64::new(0),
                verbose: AtomicBool::new(false),
                snapshot: Mutex::new(Arc::new(DiagnosticsSnapshot::default())),
            }),
        }
    }

    fn send(&self, msg: Msg) {
        if let Some(tx) = &self.tx {
            match tx.try_send(msg) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    self.shared.dropped.fetch_add(1, Ordering::Relaxed);
                }
                Err(TrySendError::Disconnected(_)) => {}
            }
        }
    }

    pub fn emit(&self, event: Event) {
        self.send(Msg::Event(event));
    }

    /// Write a caller-shaped line into the same session file.
    ///
    /// `payload` must be a JSON object carrying its own `type`; the writer adds `v`, `ts` and
    /// `seq`. This exists so a downstream tool with its own event vocabulary -- the portal test
    /// bench and its `bench/1` profile -- can share one NDJSON file, one sequence and one
    /// writer thread with the bus events, instead of producing a second log that has to be
    /// correlated by timestamp afterwards.
    pub fn emit_line(&self, payload: serde_json::Value) {
        debug_assert!(payload.is_object(), "a report line must be a JSON object");
        self.send(Msg::Line(payload));
    }

    pub fn is_verbose(&self) -> bool {
        self.shared.verbose.load(Ordering::Relaxed)
    }

    pub fn set_verbose(&self, verbose: bool) {
        self.shared.verbose.store(verbose, Ordering::Relaxed);
    }

    /// Latest diagnostics snapshot (refreshed ~1 Hz by the writer).
    pub fn snapshot(&self) -> Arc<DiagnosticsSnapshot> {
        self.shared.snapshot.lock().unwrap().clone()
    }

    /// Ask the writer to write a summary now; blocks briefly for the path.
    pub fn write_summary_now(&self) -> Option<PathBuf> {
        let tx = self.tx.as_ref()?;
        let (reply_tx, reply_rx) = sync_channel(1);
        tx.try_send(Msg::WriteSummary(reply_tx)).ok()?;
        reply_rx.recv_timeout(Duration::from_secs(5)).ok()
    }

    // ------------------------------------------------ hot-path accounting

    /// Record a transmitted packet. When verbose, also logs a raw line
    /// (allocating the address string only in that case).
    pub fn packet_tx(&self, col: u8, target: i8, addr: &str, bytes: usize, needs_ack: bool) {
        if self.is_verbose() {
            self.send(Msg::Event(Event::PacketTx {
                col,
                portal: target,
                addr: addr.to_owned(),
                bytes: bytes as u32,
                needs_ack,
            }));
        }
        self.send(Msg::Tx {
            col,
            target,
            needs_ack,
            bytes: bytes as u32,
        });
    }

    pub fn packet_rx(
        &self,
        col: u8,
        source: i8,
        kind: RxKind,
        bytes: usize,
        latency_ms: Option<f32>,
    ) {
        if self.is_verbose() {
            self.send(Msg::Event(Event::PacketRx {
                col,
                source,
                kind: kind.as_str().to_owned(),
                bytes: bytes as u32,
                latency_ms,
            }));
        }
        self.send(Msg::Rx {
            col,
            source,
            kind,
            bytes: bytes as u32,
            latency_ms,
        });
    }

    pub fn outbox_depth(&self, col: u8, depth: usize) {
        self.send(Msg::OutboxDepth {
            col,
            depth: depth as u32,
        });
    }
}

/// Owns the writer thread; `shutdown()` drains, writes session_end + final
/// summary, and returns the summary path.
pub struct ReporterHandle {
    tx: SyncSender<Msg>,
    join: Option<std::thread::JoinHandle<Option<PathBuf>>>,
}

impl ReporterHandle {
    pub fn shutdown(mut self) -> Option<PathBuf> {
        let _ = self.tx.try_send(Msg::Shutdown { reason: "clean" });
        self.join.take().and_then(|j| j.join().ok().flatten())
    }
}

impl Drop for ReporterHandle {
    fn drop(&mut self) {
        if let Some(join) = self.join.take() {
            let _ = self.tx.try_send(Msg::Shutdown { reason: "clean" });
            let _ = join.join();
        }
    }
}
