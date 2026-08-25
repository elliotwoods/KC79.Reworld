//! Replacing the bootloader itself, over RS485, from inside the running application.
//!
//! # Why the application has to do this
//!
//! A bootloader cannot overwrite the flash it is executing from. So the only in-band route
//! from the fielded 24 kB v4/v5 bootloader to the 16 kB v6 one runs the other way round:
//! the *application* receives the new bootloader image, holds it in RAM, and programs
//! pages 0-7 from a routine that is itself running from RAM. That is what the `blimg`
//! verbs on the application's control plane are for, and this module is their host half.
//!
//! ```text
//!   {"blimg": {"begin":  [len, crc32c]}}   -> ACK, once the staging area is ready
//!   {"blimg": {"data":   [offset, bin]}}   -> ACK per chunk
//!   {"blimg": {"commit": [stay]}}          -> ACK, then the pages are written and the MCU resets
//!   {"blimg": {"abort":  nil}}
//!   {"blimg": {"q":      nil}}
//! ```
//!
//! Every one of those is **unicast and acknowledged**, which is the opposite of the
//! application-image path and for a good reason: an application-image frame that goes
//! missing costs a repair round, and a bootloader-image frame that goes missing costs a
//! board that no longer boots.
//!
//! # The window
//!
//! `commit` erases and reprograms the bootloader bank. For roughly **half a second** from
//! the acknowledgement of that request there is no valid bootloader in flash, and a board
//! that loses power inside that window cannot be recovered over RS485 by anything -- it
//! needs an ST-Link on the SWD header. Nothing here can shorten that window; what it can
//! do is refuse to open it for an image that was never going to work, which is what
//! [`validate`] is for.
//!
//! # Confirming
//!
//! Success is not the `commit` acknowledgement -- that is sent *before* the write. It is a
//! later `bl status` reporting protocol version [`layout::BL_PROTO_VERSION`], which only a
//! v6 bootloader can answer. When `commit` was told not to stay resident, the board is
//! back in its application by then, so the settle phase sends the ordinary announce words
//! to recall it before asking.

use std::time::{Duration, Instant};

use router_proto::bootloader::{self, BlReply, BlSelector};
use router_proto::envelope::encode_envelope_trailer;
use router_proto::replies::{classify_reply, Reply};
use router_proto::value::{key, map};
use router_proto::{crc32c, layout, Envelope, Value};

use crate::fw_session::FwBus;
use crate::fw_update;
use crate::rs485::{Packet, Payload};

/// The body key the application's bootloader-image verbs live under.
pub const KEY: &str = "blimg";

/// The v6 bootloader bank: pages 0-7. An image larger than this would run into the
/// application it is supposed to start.
pub const MAX_BYTES: usize = layout::BOOTLOADER_BYTES as usize;

/// The string every Portal bootloader prints on its debug UART at startup.
///
/// Checked because the other three checks pass for any Cortex-M image linked at
/// `0x08000000` -- including an *application* built with the `no_bootloader` linker script,
/// which is a real file sitting in a real `.pio/build/` directory with a plausible name.
/// Writing that over pages 0-7 produces a board that comes up, runs nothing, and answers
/// nothing.
pub const BANNER: &[u8] = b"Bootloader v";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BlImageError {
    #[error("bootloader image is empty")]
    Empty,
    #[error("bootloader image is {bytes} bytes; the bootloader bank is {limit} bytes")]
    TooLarge { bytes: usize, limit: usize },
    #[error("initial stack pointer 0x{sp:08X} is not inside SRAM")]
    BadStackPointer { sp: u32 },
    #[error(
        "reset vector 0x{vector:08X} is not a Thumb entry point inside the first {limit} bytes \
         of flash"
    )]
    BadResetVector { vector: u32, limit: usize },
    #[error(
        "the image does not contain the string \"{}\"; it is not a Portal bootloader",
        String::from_utf8_lossy(BANNER)
    )]
    NoBanner,
}

/// Everything that can be checked about a bootloader image without programming it.
///
/// The first three are structural: an image whose vector table is wrong is one the MCU
/// will fault on before any of this module's code runs again. The fourth is what
/// distinguishes a bootloader from every other image that would pass the first three.
pub fn validate(image: &[u8]) -> Result<(), BlImageError> {
    if image.is_empty() {
        return Err(BlImageError::Empty);
    }
    if image.len() > MAX_BYTES {
        return Err(BlImageError::TooLarge {
            bytes: image.len(),
            limit: MAX_BYTES,
        });
    }
    let word = |at: usize| {
        u32::from_le_bytes([image[at], image[at + 1], image[at + 2], image[at + 3]])
    };
    if image.len() < 8 {
        return Err(BlImageError::BadResetVector {
            vector: 0,
            limit: MAX_BYTES,
        });
    }
    let sp = word(0);
    if !(layout::RAM_BASE..=layout::RAM_END).contains(&sp) {
        return Err(BlImageError::BadStackPointer { sp });
    }
    let vector = word(4);
    let entry = vector & !1;
    if vector & 1 == 0 || !(layout::FLASH_BASE..layout::FLASH_BASE + MAX_BYTES as u32).contains(&entry)
    {
        return Err(BlImageError::BadResetVector {
            vector,
            limit: MAX_BYTES,
        });
    }
    if !image.windows(BANNER.len()).any(|window| window == BANNER) {
        return Err(BlImageError::NoBanner);
    }
    Ok(())
}

/// Validate, then pad to a whole double-word -- flash programs 64 bits at a time, and a
/// short final write reads whatever follows the sender's buffer.
pub fn prepare(image: &[u8]) -> Result<Vec<u8>, BlImageError> {
    validate(image)?;
    let mut out = image.to_vec();
    let remainder = out.len() % layout::FLASH_GRANULE;
    if remainder != 0 {
        out.resize(out.len() + (layout::FLASH_GRANULE - remainder), 0xFF);
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct BlUpdateParams {
    /// The board. Unicast throughout: this is never a fleet operation.
    pub id: i8,
    /// Ask the new bootloader to stay resident after the reset instead of starting the
    /// application. Cheaper to confirm, and the right choice when the application is about
    /// to be replaced too.
    pub stay: bool,
    pub chunk_bytes: usize,
    /// Gap after each frame, so the application's reply is not met head-on by the next one.
    pub gap_ms: u32,
    /// Time allowed for the board to leave its bootloader and reach the application.
    pub escape_ms: u32,
    pub ack_timeout_ms: u32,
    /// `begin` may erase or clear a staging area before it answers.
    pub begin_timeout_ms: u32,
    pub commit_timeout_ms: u32,
    /// Reset, new bootloader startup, and -- when `stay` is false -- the recall that brings
    /// it back out of the application.
    pub settle_ms: u32,
    pub confirm_timeout_ms: u32,
    /// Attempts per frame before the update is abandoned.
    pub max_attempts: u8,
}

impl Default for BlUpdateParams {
    fn default() -> Self {
        Self {
            id: 1,
            stay: true,
            chunk_bytes: 128,
            gap_ms: 4,
            escape_ms: 1_500,
            ack_timeout_ms: 500,
            begin_timeout_ms: 2_000,
            commit_timeout_ms: 3_000,
            settle_ms: 3_000,
            confirm_timeout_ms: 1_000,
            max_attempts: 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlPhase {
    /// Leave any bootloader the board may be sitting in: only the application can do this.
    Escape,
    Begin,
    Data { next: usize },
    Commit,
    /// The reset, the new bootloader's startup, and the recall when one is needed.
    Settle,
    Confirm,
    Done,
}

#[derive(Debug, Clone)]
pub struct BlUpdateProgress {
    pub phase: BlPhase,
    pub fraction: f32,
    pub detail: String,
    pub chunk: usize,
    pub chunks: usize,
    /// Attempts spent on the frame currently outstanding.
    pub attempt: u8,
    pub packets_queued: usize,
    pub done: bool,
    pub ok: bool,
}

/// What is outstanding, and what would answer it.
#[derive(Debug, Clone, Copy)]
struct Pending {
    seq: u8,
    deadline: Instant,
    /// True while waiting for a `bl status` rather than an application ACK.
    confirming: bool,
}

pub struct BootloaderUpdate {
    params: BlUpdateParams,
    image: Vec<u8>,
    crc32: u32,
    chunks: usize,
    phase: BlPhase,
    pending: Option<Pending>,
    attempt: u8,
    /// When the phase that is only a wait may end.
    wait_until: Option<Instant>,
    queued_phase_work: bool,
    detail: String,
    ok: bool,
    seq: u8,
    queued: usize,
}

impl BootloaderUpdate {
    pub fn new(image: &[u8], params: BlUpdateParams) -> Result<Self, BlImageError> {
        let image = prepare(image)?;
        let chunk = params.chunk_bytes.clamp(layout::FLASH_GRANULE, layout::BL_CHUNK_MAX);
        let chunk = chunk - (chunk % layout::FLASH_GRANULE);
        let params = BlUpdateParams {
            chunk_bytes: chunk,
            ..params
        };
        Ok(Self {
            crc32: crc32c(&image),
            chunks: image.len().div_ceil(chunk),
            image,
            params,
            phase: BlPhase::Escape,
            pending: None,
            attempt: 0,
            wait_until: None,
            queued_phase_work: false,
            detail: String::new(),
            ok: false,
            seq: 0,
            queued: 0,
        })
    }

    pub fn image_len(&self) -> usize {
        self.image.len()
    }

    pub fn image_crc32(&self) -> u32 {
        self.crc32
    }

    pub fn tick(&mut self, bus: &dyn FwBus, now: Instant, envelopes: &[Envelope]) -> BlUpdateProgress {
        let answer = self.ingest(envelopes);
        match answer {
            Some(Answer::Ack) => self.advance(now),
            Some(Answer::Nack) => self.retry(bus, now, "the board refused the frame"),
            Some(Answer::Confirmed) => {
                self.pending = None;
                self.finish(true, "the board is running bootloader v6".into());
            }
            None => {}
        }
        if let Some(pending) = self.pending {
            if now >= pending.deadline {
                self.retry(bus, now, "no reply");
            }
        }
        if self.pending.is_none() {
            self.drive(bus, now);
        }
        self.progress()
    }

    pub fn abort(&mut self, bus: &dyn FwBus) {
        // Tell the board to drop its staging area rather than leaving it to time out with a
        // half-received bootloader image in RAM.
        let seq = self.next_seq();
        let body = verb_body("abort", Value::Nil);
        self.send(bus, encode_envelope_trailer(self.params.id, &body, seq));
        self.pending = None;
        self.finish(false, "aborted".into());
    }

    // ---------------------------------------------------------------- replies

    fn ingest(&mut self, envelopes: &[Envelope]) -> Option<Answer> {
        let pending = self.pending?;
        for envelope in envelopes {
            if !envelope.trailer.acceptable() || envelope.source != self.params.id {
                continue;
            }
            if envelope
                .trailer
                .seq()
                .is_some_and(|seq| seq != pending.seq)
            {
                continue;
            }
            match classify_reply(&envelope.body) {
                Reply::Ack(true) if !pending.confirming => return Some(Answer::Ack),
                Reply::Ack(false) if !pending.confirming => return Some(Answer::Nack),
                Reply::Bootloader(BlReply::Status(status)) if pending.confirming => {
                    if bootloader::speaks_v6(&status) {
                        return Some(Answer::Confirmed);
                    }
                    return Some(Answer::Nack);
                }
                _ => {}
            }
        }
        None
    }

    fn advance(&mut self, now: Instant) {
        self.pending = None;
        self.attempt = 0;
        match self.phase {
            BlPhase::Begin => {
                self.phase = BlPhase::Data { next: 0 };
            }
            BlPhase::Data { next } => {
                let next = next + 1;
                self.phase = if next < self.chunks {
                    BlPhase::Data { next }
                } else {
                    // Every chunk acknowledged, and only now: `commit` writes whatever the
                    // board holds, so a gap here is a bootloader with a hole in it.
                    BlPhase::Commit
                };
            }
            BlPhase::Commit => {
                self.phase = BlPhase::Settle;
                self.queued_phase_work = false;
                self.wait_until =
                    Some(now + Duration::from_millis(u64::from(self.params.settle_ms)));
            }
            _ => {}
        }
    }

    fn retry(&mut self, bus: &dyn FwBus, now: Instant, why: &str) {
        self.pending = None;
        self.attempt += 1;
        if self.attempt >= self.params.max_attempts {
            let phase = self.phase;
            self.finish(
                false,
                format!(
                    "{why}: gave up after {} attempts in {}",
                    self.attempt,
                    phase_label(phase)
                ),
            );
            return;
        }
        self.drive(bus, now);
    }

    // ----------------------------------------------------------------- phases

    fn drive(&mut self, bus: &dyn FwBus, now: Instant) {
        match self.phase {
            // Unanswered on purpose: the board may be in its bootloader, in which case this
            // starts the application, or already in the application, in which case it is an
            // unknown verb it ignores. Neither case is distinguishable from here, and both
            // are fine.
            BlPhase::Escape => {
                if !self.queued_phase_work {
                    let seq = self.next_seq();
                    self.send(bus, bootloader::run(self.params.id, seq));
                    self.queued_phase_work = true;
                    self.wait_until =
                        Some(now + Duration::from_millis(u64::from(self.params.escape_ms)));
                    return;
                }
                if self.wait_until.is_some_and(|until| now >= until) {
                    self.phase = BlPhase::Begin;
                    self.queued_phase_work = false;
                }
            }
            BlPhase::Begin => {
                let body = verb_body(
                    "begin",
                    Value::Array(vec![
                        Value::from(self.image.len() as u32),
                        Value::from(self.crc32),
                    ]),
                );
                self.request(bus, now, body, self.params.begin_timeout_ms, false);
            }
            BlPhase::Data { next } => {
                let chunk = self.params.chunk_bytes;
                let start = next * chunk;
                let end = (start + chunk).min(self.image.len());
                let body = verb_body(
                    "data",
                    Value::Array(vec![
                        Value::from(start as u32),
                        Value::Binary(self.image[start..end].to_vec()),
                    ]),
                );
                self.request(bus, now, body, self.params.ack_timeout_ms, false);
            }
            BlPhase::Commit => {
                let body = verb_body("commit", Value::Array(vec![Value::from(self.params.stay)]));
                self.request(bus, now, body, self.params.commit_timeout_ms, false);
            }
            BlPhase::Settle => {
                if !self.queued_phase_work {
                    if !self.params.stay {
                        // The board is back in its application, so the only thing that will
                        // answer a `bl status` is a bootloader it has to be recalled into.
                        for step in fw_update::announce_steps(&Default::default()) {
                            let packet = fw_update::step_packet(step, &[]);
                            self.send_packet(bus, packet);
                        }
                    }
                    self.queued_phase_work = true;
                    return;
                }
                if bus.outbox_len() == 0 && self.wait_until.is_some_and(|until| now >= until) {
                    self.phase = BlPhase::Confirm;
                    self.attempt = 0;
                }
            }
            BlPhase::Confirm => {
                let seq = self.next_seq();
                self.send(bus, bootloader::status(self.params.id, BlSelector::None, seq));
                self.pending = Some(Pending {
                    seq,
                    deadline: now
                        + Duration::from_millis(u64::from(self.params.confirm_timeout_ms)),
                    confirming: true,
                });
            }
            BlPhase::Done => {}
        }
    }

    fn request(
        &mut self,
        bus: &dyn FwBus,
        now: Instant,
        body: Value,
        timeout_ms: u32,
        confirming: bool,
    ) {
        let seq = self.next_seq();
        self.send(bus, encode_envelope_trailer(self.params.id, &body, seq));
        self.pending = Some(Pending {
            seq,
            deadline: now + Duration::from_millis(u64::from(timeout_ms)),
            confirming,
        });
    }

    fn send(&mut self, bus: &dyn FwBus, bytes: Vec<u8>) {
        let packet = Packet {
            payload: Payload::Rendered(bytes),
            target: self.params.id,
            // The same two transport rules as `crate::fw_session`: the worker treats any
            // frame from the target as an ACK and would consume the reply this module is
            // correlating by seq, and the outbox collapses same-address packets.
            address: String::new(),
            needs_ack: false,
            collateable: false,
            custom_wait_time_ms: Some(self.params.gap_ms),
            on_sent: None,
        };
        self.send_packet(bus, packet);
    }

    fn send_packet(&mut self, bus: &dyn FwBus, packet: Packet) {
        self.queued += 1;
        bus.transmit(packet);
    }

    fn next_seq(&mut self) -> u8 {
        self.seq = self.seq.wrapping_add(1);
        self.seq
    }

    fn finish(&mut self, ok: bool, detail: String) {
        self.phase = BlPhase::Done;
        self.ok = ok;
        self.detail = detail;
        self.pending = None;
    }

    fn progress(&self) -> BlUpdateProgress {
        let chunk = match self.phase {
            BlPhase::Data { next } => next,
            BlPhase::Escape | BlPhase::Begin => 0,
            _ => self.chunks,
        };
        BlUpdateProgress {
            phase: self.phase,
            fraction: match self.phase {
                BlPhase::Escape => 0.0,
                BlPhase::Begin => 0.05,
                BlPhase::Data { next } => 0.1 + 0.75 * (next as f32 / self.chunks.max(1) as f32),
                BlPhase::Commit => 0.87,
                BlPhase::Settle => 0.92,
                BlPhase::Confirm => 0.97,
                BlPhase::Done => 1.0,
            },
            detail: if self.detail.is_empty() {
                phase_label(self.phase).to_string()
            } else {
                self.detail.clone()
            },
            chunk,
            chunks: self.chunks,
            attempt: self.attempt,
            packets_queued: self.queued,
            done: self.phase == BlPhase::Done,
            ok: self.phase == BlPhase::Done && self.ok,
        }
    }
}

enum Answer {
    Ack,
    Nack,
    Confirmed,
}

fn verb_body(verb: &str, payload: Value) -> Value {
    map(vec![(key(KEY), map(vec![(key(verb), payload)]))])
}

fn phase_label(phase: BlPhase) -> &'static str {
    match phase {
        BlPhase::Escape => "leaving the bootloader",
        BlPhase::Begin => "opening the staging area",
        BlPhase::Data { .. } => "sending the bootloader image",
        BlPhase::Commit => "programming pages 0-7 (do not remove power)",
        BlPhase::Settle => "waiting for the board to come back",
        BlPhase::Confirm => "confirming the new bootloader",
        BlPhase::Done => "done",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fw_session::MockBus;
    use router_proto::envelope::{decode_envelope, encode_reply_trailer};

    /// A plausible bootloader image: vector table, banner, and nothing else that matters.
    fn bootloader_image(bytes: usize) -> Vec<u8> {
        let mut image = vec![0u8; bytes];
        image[..4].copy_from_slice(&layout::RAM_END.to_le_bytes());
        image[4..8].copy_from_slice(&((layout::FLASH_BASE + 0x241) | 1).to_le_bytes());
        let banner = b"Bootloader v2026-08-25 (proto 6)";
        image[0x200..0x200 + banner.len()].copy_from_slice(banner);
        image
    }

    struct Harness {
        update: BootloaderUpdate,
        bus: MockBus,
        now: Instant,
        inbox: Vec<Envelope>,
        seen: usize,
        requests: Vec<(String, Envelope)>,
    }

    impl Harness {
        fn new(params: BlUpdateParams) -> Self {
            Self {
                update: BootloaderUpdate::new(&bootloader_image(1_024), params).expect("image"),
                bus: MockBus::new(),
                now: Instant::now(),
                inbox: Vec::new(),
                seen: 0,
                requests: Vec::new(),
            }
        }

        fn tick(&mut self) -> BlUpdateProgress {
            let inbox = std::mem::take(&mut self.inbox);
            let progress = self.update.tick(&self.bus, self.now, &inbox);
            let sent = self.bus.sent();
            for packet in &sent[self.seen..] {
                if let Ok(envelope) = decode_envelope(&packet.bytes) {
                    self.requests.push((request_name(&envelope), envelope));
                }
            }
            self.seen = sent.len();
            progress
        }

        fn advance(&mut self, ms: u64) {
            self.now += Duration::from_millis(ms);
        }

        /// The seq of the most recent request, which is what a reply has to echo.
        fn last_seq(&self) -> u8 {
            self.requests
                .last()
                .and_then(|(_, envelope)| envelope.trailer.seq())
                .expect("a trailered request")
        }

        fn last_name(&self) -> String {
            self.requests
                .last()
                .map(|(name, _)| name.clone())
                .unwrap_or_default()
        }

        fn ack(&mut self, value: bool) {
            let seq = self.last_seq();
            let bytes = encode_reply_trailer(self.update.params.id, &Value::Boolean(value), seq);
            self.inbox.push(decode_envelope(&bytes).unwrap());
        }

        fn status(&mut self, version: u8) {
            let seq = self.last_seq();
            let body = map(vec![(
                key("bl"),
                map(vec![
                    (key("q"), Value::from("status")),
                    (key("v"), Value::from(version)),
                    (key("id"), Value::from(self.update.params.id)),
                    (key("base"), Value::from(layout::APP_BASE)),
                    (key("st"), Value::from(3)),
                ]),
            )]);
            let bytes = encode_reply_trailer(self.update.params.id, &body, seq);
            self.inbox.push(decode_envelope(&bytes).unwrap());
        }

        fn names(&self) -> Vec<String> {
            self.requests.iter().map(|(name, _)| name.clone()).collect()
        }
    }

    /// Name a request by what it asks: `blimg` verb, `bl` verb, or the bare announce word.
    fn request_name(envelope: &Envelope) -> String {
        if let Some(word) = envelope.body.as_str() {
            return word.to_string();
        }
        let Value::Map(entries) = &envelope.body else {
            return "?".into();
        };
        for (outer, inner) in entries {
            let Value::Map(fields) = inner else { continue };
            match outer.as_str() {
                Some(KEY) => {
                    if let Some((verb, _)) = fields.first() {
                        return format!("blimg.{}", verb.as_str().unwrap_or("?"));
                    }
                }
                Some("bl") => {
                    if let Some(verb) = fields
                        .iter()
                        .find(|(k, _)| k.as_str() == Some("q"))
                        .and_then(|(_, v)| v.as_str())
                    {
                        return format!("bl.{verb}");
                    }
                }
                _ => {}
            }
        }
        "?".into()
    }

    /// Drive a whole successful update, acknowledging everything.
    fn run_happy(h: &mut Harness) -> BlUpdateProgress {
        let mut progress = h.tick();
        for _ in 0..500 {
            if progress.done {
                break;
            }
            match h.last_name().as_str() {
                name if name.starts_with("blimg.") => h.ack(true),
                "bl.status" => h.status(layout::BL_PROTO_VERSION),
                _ => {}
            }
            h.advance(20);
            progress = h.tick();
        }
        progress
    }

    #[test]
    fn the_packet_sequence_is_escape_begin_every_chunk_commit_then_a_status() {
        let mut h = Harness::new(BlUpdateParams::default());
        let progress = run_happy(&mut h);
        assert!(progress.done && progress.ok, "{}", progress.detail);

        let names = h.names();
        assert_eq!(names.first().map(String::as_str), Some("bl.run"), "escape");
        assert_eq!(names[1], "blimg.begin");
        let data: Vec<&String> = names.iter().filter(|name| *name == "blimg.data").collect();
        assert_eq!(data.len(), 8, "1024 bytes at 128 per chunk");
        assert_eq!(names[names.len() - 2], "blimg.commit");
        assert_eq!(names.last().map(String::as_str), Some("bl.status"));

        // The offsets are gapless, in order, and double-word aligned.
        let mut expected = 0u32;
        for (name, envelope) in &h.requests {
            if name != "blimg.data" {
                continue;
            }
            let Value::Array(fields) = payload(envelope, "data") else {
                panic!("data payload is not an array")
            };
            let offset = fields[0].as_u64().unwrap() as u32;
            assert_eq!(offset, expected);
            assert!(offset.is_multiple_of(layout::FLASH_GRANULE as u32));
            expected += fields[1].as_slice().unwrap().len() as u32;
        }
        assert_eq!(expected as usize, h.update.image_len());

        // `begin` declared the length and CRC of exactly those bytes.
        let (_, begin) = h
            .requests
            .iter()
            .find(|(name, _)| name == "blimg.begin")
            .unwrap();
        let Value::Array(fields) = payload(begin, "begin") else {
            panic!()
        };
        assert_eq!(fields[0].as_u64(), Some(h.update.image_len() as u64));
        assert_eq!(fields[1].as_u64(), Some(u64::from(h.update.image_crc32())));
    }

    #[test]
    fn commit_is_sent_only_after_every_chunk_is_acknowledged() {
        let mut h = Harness::new(BlUpdateParams::default());
        let mut acked = 0usize;
        let mut progress = h.tick();
        for _ in 0..500 {
            if progress.done {
                break;
            }
            let name = h.last_name();
            if name == "blimg.commit" {
                assert_eq!(acked, 8, "commit after only {acked} acknowledged chunks");
            }
            match name.as_str() {
                "blimg.data" => {
                    acked += 1;
                    h.ack(true);
                }
                name if name.starts_with("blimg.") => h.ack(true),
                "bl.status" => h.status(layout::BL_PROTO_VERSION),
                _ => {}
            }
            h.advance(20);
            progress = h.tick();
        }
        assert!(progress.done && progress.ok);
    }

    #[test]
    fn a_nacked_chunk_is_resent_and_a_third_refusal_ends_the_update() {
        let mut h = Harness::new(BlUpdateParams::default());
        let mut refusals = 0usize;
        let mut progress = h.tick();
        for _ in 0..500 {
            if progress.done {
                break;
            }
            match h.last_name().as_str() {
                // Chunk 3 is refused every time.
                "blimg.data" if data_offset(&h) == 3 * 128 => {
                    refusals += 1;
                    h.ack(false);
                }
                name if name.starts_with("blimg.") => h.ack(true),
                "bl.status" => h.status(layout::BL_PROTO_VERSION),
                _ => {}
            }
            h.advance(20);
            progress = h.tick();
        }
        assert!(progress.done && !progress.ok);
        assert_eq!(refusals, 3, "sent three times before giving up");
        assert!(
            !h.names().iter().any(|name| name == "blimg.commit"),
            "an incomplete image must never be committed"
        );
    }

    /// The same retry budget applies to silence, which is what a board mid-reset looks like.
    #[test]
    fn silence_is_retried_the_same_number_of_times() {
        let mut h = Harness::new(BlUpdateParams::default());
        let mut progress = h.tick();
        for _ in 0..500 {
            if progress.done {
                break;
            }
            if h.last_name() == "blimg.begin" {
                // Answer nothing at all.
            } else if h.last_name().starts_with("blimg.") {
                h.ack(true);
            }
            h.advance(600);
            progress = h.tick();
        }
        assert!(progress.done && !progress.ok);
        assert_eq!(
            h.names().iter().filter(|name| *name == "blimg.begin").count(),
            3
        );
        assert!(progress.detail.contains("no reply"), "{}", progress.detail);
    }

    /// A board that comes back on the old bootloader is a failed update, not a successful
    /// one -- the commit acknowledgement is sent before the write, so it proves nothing.
    #[test]
    fn a_board_that_still_reports_the_old_protocol_is_not_confirmed() {
        let mut h = Harness::new(BlUpdateParams::default());
        let mut progress = h.tick();
        for _ in 0..500 {
            if progress.done {
                break;
            }
            match h.last_name().as_str() {
                name if name.starts_with("blimg.") => h.ack(true),
                "bl.status" => h.status(5),
                _ => {}
            }
            h.advance(20);
            progress = h.tick();
        }
        assert!(progress.done && !progress.ok);
    }

    /// When the board was told not to stay resident it is running its application by the
    /// time anything can be asked of it, so the settle phase has to recall it first.
    #[test]
    fn a_non_staying_commit_recalls_the_board_before_confirming() {
        let mut h = Harness::new(BlUpdateParams {
            stay: false,
            ..Default::default()
        });
        let progress = run_happy(&mut h);
        assert!(progress.done && progress.ok, "{}", progress.detail);
        let names = h.names();
        let commit = names.iter().position(|name| name == "blimg.commit").unwrap();
        let status = names.iter().rposition(|name| name == "bl.status").unwrap();
        let recalls = names[commit..status]
            .iter()
            .filter(|name| *name == "FW!KC79")
            .count();
        assert!(recalls > 10, "only {recalls} recall words before the status");
    }

    #[test]
    fn every_packet_is_unacked_and_uncollateable() {
        let mut h = Harness::new(BlUpdateParams::default());
        run_happy(&mut h);
        for packet in h.bus.sent() {
            assert!(!packet.needs_ack);
            assert!(!packet.collateable);
            assert!(packet.address.is_empty());
        }
    }

    // ------------------------------------------------------------- validation

    #[test]
    fn an_image_that_is_not_a_bootloader_is_refused_before_anything_is_sent() {
        // Too large for pages 0-7.
        assert_eq!(
            validate(&bootloader_image(MAX_BYTES + 8)),
            Err(BlImageError::TooLarge {
                bytes: MAX_BYTES + 8,
                limit: MAX_BYTES
            })
        );

        // A stack pointer outside SRAM: the first instruction after reset would fault.
        let mut bad = bootloader_image(1_024);
        bad[..4].copy_from_slice(&0x0800_0000u32.to_le_bytes());
        assert!(matches!(
            validate(&bad),
            Err(BlImageError::BadStackPointer { .. })
        ));

        // A reset vector without the Thumb bit is not a Cortex-M entry point.
        let mut bad = bootloader_image(1_024);
        bad[4] &= !1;
        assert!(matches!(
            validate(&bad),
            Err(BlImageError::BadResetVector { .. })
        ));

        // A reset vector inside the *application* bank: an application image, mislabelled.
        let mut bad = bootloader_image(1_024);
        bad[4..8].copy_from_slice(&((layout::APP_BASE + 0x241) | 1).to_le_bytes());
        assert!(matches!(
            validate(&bad),
            Err(BlImageError::BadResetVector { .. })
        ));

        // Structurally perfect, and not a bootloader: the `no_bootloader` application build
        // links at 0x08000000 and passes every check above.
        let mut no_banner = bootloader_image(1_024);
        for byte in no_banner.iter_mut().skip(0x200).take(64) {
            *byte = 0;
        }
        assert_eq!(validate(&no_banner), Err(BlImageError::NoBanner));

        assert_eq!(validate(&[]), Err(BlImageError::Empty));
        assert!(BootloaderUpdate::new(&no_banner, BlUpdateParams::default()).is_err());
    }

    #[test]
    fn a_short_image_is_padded_to_a_whole_double_word() {
        let image = bootloader_image(1_020);
        let prepared = prepare(&image).unwrap();
        assert_eq!(prepared.len(), 1_024);
        assert_eq!(&prepared[1_020..], &[0xFF; 4]);
        assert_eq!(&prepared[..1_020], &image[..]);
    }

    fn payload(envelope: &Envelope, verb: &str) -> Value {
        let Value::Map(entries) = &envelope.body else {
            panic!("not a map")
        };
        let (_, inner) = entries
            .iter()
            .find(|(k, _)| k.as_str() == Some(KEY))
            .expect("blimg");
        let Value::Map(fields) = inner else {
            panic!("blimg is not a map")
        };
        fields
            .iter()
            .find(|(k, _)| k.as_str() == Some(verb))
            .map(|(_, v)| v.clone())
            .expect("verb")
    }

    fn data_offset(h: &Harness) -> u32 {
        let (_, envelope) = h.requests.last().unwrap();
        let Value::Array(fields) = payload(envelope, "data") else {
            panic!()
        };
        fields[0].as_u64().unwrap() as u32
    }
}
