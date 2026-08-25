//! RS485 connection facade owned by a Column: worker lifecycle, reconnect
//! (20 s retry from stored settings, like `RS485::update`), inbox draining,
//! and outbox access.

pub mod device;
pub mod outbox;
pub mod worker;

use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use router_proto::Envelope;
use router_report::Reporter;
use serde_json::Value as Json;

pub use device::{create_device, list_serial_ports, MockDevice, SerialDevice};
pub use outbox::{Packet, Payload};
pub use worker::Rs485Params;

const RETRY_PERIOD: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Default)]
pub struct Rs485Stats {
    pub connected: bool,
    pub device_description: String,
    pub tx_count: u64,
    pub rx_count: u64,
    pub ack_timeouts: u64,
    pub decode_errors: u64,
    pub outbox_size: usize,
    pub last_rx_age_ms: Option<u64>,
    pub last_tx_age_ms: Option<u64>,
}

struct Worker {
    shared: Arc<worker::Shared>,
    join: Option<std::thread::JoinHandle<()>>,
    description: String,
}

pub struct Rs485 {
    pub params: Rs485Params,
    col: u8,
    reporter: Reporter,
    settings: Option<Json>,
    worker: Option<Worker>,
    last_attempt: Option<Instant>,
    inbox_tx: Sender<Envelope>,
    inbox_rx: Receiver<Envelope>,
}

impl Rs485 {
    pub fn new(col: u8, reporter: Reporter) -> Self {
        let (inbox_tx, inbox_rx) = channel();
        Self {
            params: Rs485Params::default(),
            col,
            reporter,
            settings: None,
            worker: None,
            last_attempt: None,
            inbox_tx,
            inbox_rx,
        }
    }

    /// `RS485::deserialise`: store the connection settings and connect.
    pub fn open_from_settings(&mut self, settings: Json) {
        self.params.apply_settings(&settings);
        self.settings = Some(settings);
        self.try_open();
    }

    pub fn open_device(&mut self, device: Box<dyn SerialDevice>) {
        self.close();
        let description = format!("{} {}", device.type_name(), device.address_string());
        let shared = Arc::new(worker::Shared::new(self.params));
        let join = worker::spawn(
            device,
            worker::WorkerContext {
                col: self.col,
                shared: shared.clone(),
                inbox: self.inbox_tx.clone(),
                reporter: self.reporter.clone(),
            },
        );
        self.worker = Some(Worker {
            shared,
            join: Some(join),
            description,
        });
    }

    fn try_open(&mut self) {
        self.last_attempt = Some(Instant::now());
        let Some(settings) = &self.settings else { return };
        match create_device(settings) {
            Ok(device) => self.open_device(device),
            Err(e) => {
                let transport = settings
                    .get("deviceType")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?")
                    .to_string();
                let endpoint = settings
                    .get("address")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?")
                    .to_string();
                self.reporter.emit(router_report::Event::DeviceConnect {
                    col: self.col,
                    transport,
                    endpoint,
                    ok: false,
                    error: Some(e.to_string()),
                });
            }
        }
    }

    pub fn close(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.shared.shutdown.store(true, Ordering::Release);
            if let Some(join) = worker.join {
                let _ = join.join();
            }
        }
    }

    pub fn is_connected(&self) -> bool {
        self.worker
            .as_ref()
            .map(|w| w.shared.connected.load(Ordering::Acquire))
            .unwrap_or(false)
    }

    /// Per-tick housekeeping: reap dead workers, retry connection every 20 s,
    /// push live params, and drain received envelopes.
    pub fn update(&mut self) -> Vec<Envelope> {
        // reap a dead worker (device error ended the thread); the connected
        // flag alone is not enough — a freshly spawned worker hasn't set it
        // yet, and joining a live worker would deadlock
        if let Some(worker) = &mut self.worker {
            let finished = worker.join.as_ref().map(|j| j.is_finished()).unwrap_or(true);
            if finished {
                if let Some(join) = worker.join.take() {
                    let _ = join.join();
                }
                self.worker = None;
            } else {
                *worker.shared.params.lock().unwrap() = self.params;
            }
        }

        // reconnect from stored settings
        if self.worker.is_none() && self.settings.is_some() {
            let due = self
                .last_attempt
                .map(|t| t.elapsed() >= RETRY_PERIOD)
                .unwrap_or(true);
            if due {
                self.try_open();
            }
        }

        // drain inbox
        let mut envelopes = Vec::new();
        while let Ok(envelope) = self.inbox_rx.try_recv() {
            envelopes.push(envelope);
        }
        envelopes
    }

    pub fn transmit(&self, packet: Packet) {
        if let Some(worker) = &self.worker {
            worker.shared.outbox.lock().unwrap().push(packet);
        }
    }

    pub fn remove_packets(&self, address: &str, target: i8) {
        if let Some(worker) = &self.worker {
            worker.shared.outbox.lock().unwrap().remove(address, target);
        }
    }

    pub fn clear_outbox(&self) {
        if let Some(worker) = &self.worker {
            worker.shared.outbox.lock().unwrap().clear();
        }
    }

    pub fn outbox_len(&self) -> usize {
        self.worker
            .as_ref()
            .map(|w| w.shared.outbox.lock().unwrap().len())
            .unwrap_or(0)
    }

    pub fn clear_counters(&self) {
        if let Some(worker) = &self.worker {
            worker.shared.tx_count.store(0, Ordering::Relaxed);
            worker.shared.rx_count.store(0, Ordering::Relaxed);
            worker.shared.ack_timeouts.store(0, Ordering::Relaxed);
            worker.shared.decode_errors.store(0, Ordering::Relaxed);
        }
    }

    pub fn stats(&self) -> Rs485Stats {
        let Some(worker) = &self.worker else {
            return Rs485Stats::default();
        };
        let shared = &worker.shared;
        let now = shared.started.elapsed().as_millis() as u64;
        let age = |stamp: u64| if stamp == 0 { None } else { Some(now.saturating_sub(stamp)) };
        Rs485Stats {
            connected: shared.connected.load(Ordering::Acquire),
            device_description: worker.description.clone(),
            tx_count: shared.tx_count.load(Ordering::Relaxed),
            rx_count: shared.rx_count.load(Ordering::Relaxed),
            ack_timeouts: shared.ack_timeouts.load(Ordering::Relaxed),
            decode_errors: shared.decode_errors.load(Ordering::Relaxed),
            outbox_size: shared.outbox.lock().unwrap().len(),
            last_rx_age_ms: age(shared.last_rx_ms.load(Ordering::Relaxed)),
            last_tx_age_ms: age(shared.last_tx_ms.load(Ordering::Relaxed)),
        }
    }

    /// Connection settings for reconnect / persistence.
    pub fn settings(&self) -> Option<&Json> {
        self.settings.as_ref()
    }
}

impl Drop for Rs485 {
    fn drop(&mut self) {
        self.close();
    }
}
