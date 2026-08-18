//! The per-column serial worker thread: port of `RS485::serialThreadedFunction`.
//!
//! Timing semantics preserved from the C++:
//! - RX has priority; no TX within `gap_after_last_rx_ms` (5) of the last RX.
//! - A `needs_ack` packet waits up to `response_window_ms` (300) for any
//!   frame whose *source* equals the packet's target.
//! - Broadcasts (no ACK) sleep `gap_between_broadcast_sends_ms` (100), or the
//!   packet's `custom_wait_time_ms` when set (0 = no wait).
//! - Outbox collation happens just before draining.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use router_proto::{decode_envelope, encode_frame, Envelope, FrameAccumulator, ProtoError};
use router_report::{Reporter, RxKind};

use super::device::SerialDevice;
use super::outbox::{Outbox, Packet, Payload};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rs485Params {
    pub response_window_ms: u32,
    pub gap_between_broadcast_sends_ms: u32,
    pub gap_after_last_rx_ms: u32,
    pub collate_packets: bool,
}

impl Default for Rs485Params {
    fn default() -> Self {
        Self {
            response_window_ms: 300,
            gap_between_broadcast_sends_ms: 100,
            gap_after_last_rx_ms: 5,
            collate_packets: true,
        }
    }
}

/// State shared between the worker thread and the owning `Rs485`.
pub struct Shared {
    pub outbox: Mutex<Outbox>,
    pub params: Mutex<Rs485Params>,
    pub shutdown: AtomicBool,
    pub connected: AtomicBool,
    pub tx_count: AtomicU64,
    pub rx_count: AtomicU64,
    pub ack_timeouts: AtomicU64,
    pub decode_errors: AtomicU64,
    pub outbox_peak: AtomicUsize,
    /// ms since worker start of the last rx/tx (0 = never), for heartbeats.
    pub last_rx_ms: AtomicU64,
    pub last_tx_ms: AtomicU64,
    pub started: Instant,
}

impl Shared {
    pub fn new(params: Rs485Params) -> Self {
        Self {
            outbox: Mutex::new(Outbox::default()),
            params: Mutex::new(params),
            shutdown: AtomicBool::new(false),
            connected: AtomicBool::new(false),
            tx_count: AtomicU64::new(0),
            rx_count: AtomicU64::new(0),
            ack_timeouts: AtomicU64::new(0),
            decode_errors: AtomicU64::new(0),
            outbox_peak: AtomicUsize::new(0),
            last_rx_ms: AtomicU64::new(0),
            last_tx_ms: AtomicU64::new(0),
            started: Instant::now(),
        }
    }

    fn stamp(&self) -> u64 {
        self.started.elapsed().as_millis().max(1) as u64
    }
}

pub struct WorkerContext {
    pub col: u8,
    pub shared: Arc<Shared>,
    pub inbox: Sender<Envelope>,
    pub reporter: Reporter,
}

pub fn spawn(mut device: Box<dyn SerialDevice>, ctx: WorkerContext) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name(format!("rs485-col{}", ctx.col))
        .spawn(move || run(device.as_mut(), &ctx))
        .expect("spawn rs485 worker")
}

fn run(device: &mut dyn SerialDevice, ctx: &WorkerContext) {
    let shared = &ctx.shared;
    shared.connected.store(true, Ordering::Release);
    ctx.reporter.emit(router_report::Event::DeviceConnect {
        col: ctx.col,
        transport: device.type_name().to_string(),
        endpoint: device.address_string(),
        ok: true,
        error: None,
    });

    let mut acc = FrameAccumulator::new();
    let mut last_rx = Instant::now() - Duration::from_secs(1);
    let mut disconnect: Option<(&'static str, String)> = None;

    'main: while !shared.shutdown.load(Ordering::Acquire) {
        // ---- receive (priority) ----
        let got_rx = match receive_into_inbox(device, ctx, &mut acc, &mut None) {
            Ok(got) => got,
            Err(e) => {
                disconnect = Some(("io_error", e.to_string()));
                break 'main;
            }
        };
        if got_rx {
            last_rx = Instant::now();
            continue; // C++: don't send on a loop that received
        }

        let params = *shared.params.lock().unwrap();
        if last_rx.elapsed() < Duration::from_millis(params.gap_after_last_rx_ms as u64) {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }

        // ---- send: drain the outbox ----
        let mut sent_any = false;
        loop {
            let packet = {
                let mut outbox = shared.outbox.lock().unwrap();
                if params.collate_packets {
                    outbox.collate();
                }
                let depth = outbox.len();
                if depth > shared.outbox_peak.load(Ordering::Relaxed) {
                    shared.outbox_peak.store(depth, Ordering::Relaxed);
                    ctx.reporter.outbox_depth(ctx.col, depth);
                }
                outbox.pop_front()
            };
            let Some(packet) = packet else { break };

            if let Err(e) = send_packet(device, ctx, packet, &params, &mut last_rx, &mut acc) {
                disconnect = Some(("io_error", e.to_string()));
                break 'main;
            }
            sent_any = true;

            if shared.shutdown.load(Ordering::Acquire) {
                break 'main;
            }
        }

        if !sent_any {
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    shared.connected.store(false, Ordering::Release);
    device.close();
    let (reason, error) = disconnect
        .map(|(r, e)| (r, Some(e)))
        .unwrap_or(("shutdown", None));
    ctx.reporter.emit(router_report::Event::DeviceDisconnect {
        col: ctx.col,
        reason: reason.to_string(),
        error,
    });
}

/// Read available bytes, decode frames, forward envelopes to the inbox.
/// When `ack_watch` is set, flags replies whose source matches the target.
fn receive_into_inbox(
    device: &mut dyn SerialDevice,
    ctx: &WorkerContext,
    acc: &mut FrameAccumulator,
    ack_watch: &mut Option<(i8, &mut bool)>,
) -> std::io::Result<bool> {
    let bytes = device.receive_available()?;
    if bytes.is_empty() {
        return Ok(false);
    }
    let shared = &ctx.shared;
    let mut got_frame = false;
    for result in acc.push(&bytes) {
        got_frame = true;
        match result {
            Ok(payload) => match decode_envelope(&payload) {
                Ok(envelope) => {
                    shared.rx_count.fetch_add(1, Ordering::Relaxed);
                    shared.last_rx_ms.store(shared.stamp(), Ordering::Relaxed);
                    let is_ack_match = if let Some((target, seen)) = ack_watch {
                        if envelope.source == *target {
                            **seen = true;
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                    let kind = match &envelope.body {
                        router_proto::Value::Boolean(_) => RxKind::Ack,
                        router_proto::Value::Map(_) => RxKind::Report,
                        _ => RxKind::Other,
                    };
                    // latency is stamped by the ACK-wait caller; report rx here
                    if !is_ack_match {
                        ctx.reporter
                            .packet_rx(ctx.col, envelope.source, kind, payload.len(), None);
                    }
                    let _ = ctx.inbox.send(envelope);
                }
                Err(e) => {
                    shared.decode_errors.fetch_add(1, Ordering::Relaxed);
                    let hex_prefix: String = payload
                        .iter()
                        .take(16)
                        .map(|b| format!("{b:02x}"))
                        .collect::<Vec<_>>()
                        .join(" ");
                    ctx.reporter.emit(router_report::Event::MsgpackError {
                        col: ctx.col,
                        detail: e.to_string(),
                        hex_prefix,
                    });
                }
            },
            Err(e @ (ProtoError::CobsZeroInFrame { .. } | ProtoError::CobsTruncated | ProtoError::CobsEmpty)) => {
                shared.decode_errors.fetch_add(1, Ordering::Relaxed);
                ctx.reporter.emit(router_report::Event::CobsError {
                    col: ctx.col,
                    detail: e.to_string(),
                });
            }
            Err(e) => {
                shared.decode_errors.fetch_add(1, Ordering::Relaxed);
                ctx.reporter.emit(router_report::Event::MsgpackError {
                    col: ctx.col,
                    detail: e.to_string(),
                    hex_prefix: String::new(),
                });
            }
        }
    }
    Ok(got_frame)
}

fn send_packet(
    device: &mut dyn SerialDevice,
    ctx: &WorkerContext,
    packet: Packet,
    params: &Rs485Params,
    last_rx: &mut Instant,
    acc: &mut FrameAccumulator,
) -> std::io::Result<()> {
    let shared = &ctx.shared;
    let msgpack = match packet.payload {
        Payload::Rendered(bytes) => bytes,
        Payload::Lazy(renderer) => renderer(),
    };
    let framed = encode_frame(&msgpack);
    device.transmit(&framed)?;

    shared.tx_count.fetch_add(1, Ordering::Relaxed);
    shared.last_tx_ms.store(shared.stamp(), Ordering::Relaxed);
    ctx.reporter.packet_tx(
        ctx.col,
        packet.target,
        &packet.address,
        framed.len(),
        packet.needs_ack,
    );

    if let Some(on_sent) = packet.on_sent {
        on_sent();
    }

    let sent_at = Instant::now();
    if packet.needs_ack {
        // wait for any frame from the target (source-ID match only, like C++)
        let window = Duration::from_millis(
            packet
                .custom_wait_time_ms
                .unwrap_or(params.response_window_ms) as u64,
        );
        let mut seen = false;
        while sent_at.elapsed() < window && !seen {
            let mut watch = Some((packet.target, &mut seen));
            if receive_into_inbox(device, ctx, acc, &mut watch)? {
                *last_rx = Instant::now();
            }
            if !seen {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        if seen {
            let latency_ms = sent_at.elapsed().as_secs_f32() * 1000.0;
            ctx.reporter.packet_rx(
                ctx.col,
                packet.target,
                RxKind::Ack,
                0,
                Some(latency_ms),
            );
        } else {
            shared.ack_timeouts.fetch_add(1, Ordering::Relaxed);
            if packet.target > 0 {
                ctx.reporter.emit(router_report::Event::AckTimeout {
                    col: ctx.col,
                    portal: packet.target as u8,
                    addr: packet.address.clone(),
                    waited_ms: window.as_millis() as u32,
                });
            }
        }
    } else {
        // broadcast-style gap
        let wait = packet
            .custom_wait_time_ms
            .unwrap_or(params.gap_between_broadcast_sends_ms);
        if wait > 0 {
            std::thread::sleep(Duration::from_millis(wait as u64));
        }
    }
    Ok(())
}
