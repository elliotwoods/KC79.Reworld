//! In-band firmware update for the ESP32-C3 RS485 repeaters.
//!
//! Structurally unlike the Portal path in `fw_update`, because the receiver is
//! better: the Portal bootloader demands strictly sequential offsets and has no way
//! to report a loss, so the host compensates with blind repetition. A repeater
//! records which chunks it actually received, so the host sends the image once and
//! then repairs exactly the gaps.
//!
//! ```text
//!   ota-begin  (acknowledged -- the erase blocks and drops inbound bytes)
//!   ota-data * N   (unacknowledged stream)
//!   ota-map    -> received-chunk bitmap
//!   ota-data * gaps
//!   ota-end    -> SHA-256 check and commit
//!   ota-boot
//! ```
//!
//! Rolling unicast is the default. Broadcast is faster — one pass feeds all six —
//! but it pauses every bridge at once and blacks out the whole installation for the
//! duration, so it is an explicit choice rather than the normal path.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use router_proto::repeater::{
    crc16_ccitt_false, parse_reply, request, RepeaterReply, RepeaterTarget, RepeaterVerb,
    CONTROL_PROTO_VERSION,
};
use router_proto::value::{key, map};
use router_proto::Value;

use crate::rs485::{Packet, Payload, Rs485};

/// One app slot in the stock `default.csv` partition table the field units already
/// carry, so no repartition is needed to adopt OTA.
pub const APP_SLOT_BYTES: usize = 0x140000;

/// Matches `OTA_MAX_CHUNK_BYTES` in the repeater firmware.
pub const MAX_CHUNK_BYTES: usize = 1024;

/// Matches `OTA_MAX_CHUNKS`; the receiver's bitmap cannot track more.
pub const MAX_CHUNKS: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RepeaterOtaError {
    #[error("image is {bytes} bytes; an app slot holds {limit}")]
    TooLarge { bytes: usize, limit: usize },
    #[error("chunk size must be 1..={max} bytes, got {got}")]
    BadChunkSize { got: usize, max: usize },
    #[error("image needs {chunks} chunks; the receiver tracks at most {limit}")]
    TooManyChunks { chunks: usize, limit: usize },
    #[error("image is empty")]
    Empty,
    #[error("repeater index must be 1..=6, got {0}")]
    BadIndex(u8),
}

#[derive(Debug, Clone)]
pub struct RepeaterOtaParams {
    pub chunk_bytes: usize,
    /// Gap after each data frame. The receiver writes a chunk in about 1.4 ms and
    /// the wire takes far longer than that, so this is pacing for the gateway
    /// rather than flow control for the repeater.
    pub wait_between_chunks_ms: u32,
    /// Time allowed for `ota-begin`, which erases the slot before answering. The
    /// erase runs with the cache disabled, so this is genuinely slow.
    pub begin_timeout_ms: u32,
    /// Time allowed for `ota-end`, which reads the whole slot back to hash it.
    pub end_timeout_ms: u32,
    /// Distinguishes this transfer from the last. Carried on every chunk.
    pub session: u8,
    /// Held after a streaming burst has left the outbox, before the map is asked for.
    ///
    /// An empty outbox only means the OS took the bytes; see [`drain`]. Two seconds is
    /// the margin on a real port. It is a parameter rather than a constant so a test
    /// against an instant bus does not have to wait out a margin for a wire it does not
    /// have.
    pub settle_after_burst_ms: u32,
}

impl Default for RepeaterOtaParams {
    fn default() -> Self {
        Self {
            chunk_bytes: 512,
            wait_between_chunks_ms: 2,
            begin_timeout_ms: 8000,
            end_timeout_ms: 8000,
            session: 1,
            settle_after_burst_ms: 2000,
        }
    }
}

/// Split of an image into wire chunks, shared by the initial pass and any repair.
#[derive(Debug, Clone)]
pub struct RepeaterImage {
    bytes: Vec<u8>,
    chunk_bytes: usize,
    sha256: [u8; 32],
}

impl RepeaterImage {
    pub fn new(bytes: Vec<u8>, chunk_bytes: usize) -> Result<Self, RepeaterOtaError> {
        if bytes.is_empty() {
            return Err(RepeaterOtaError::Empty);
        }
        if bytes.len() > APP_SLOT_BYTES {
            return Err(RepeaterOtaError::TooLarge {
                bytes: bytes.len(),
                limit: APP_SLOT_BYTES,
            });
        }
        if chunk_bytes == 0 || chunk_bytes > MAX_CHUNK_BYTES {
            return Err(RepeaterOtaError::BadChunkSize {
                got: chunk_bytes,
                max: MAX_CHUNK_BYTES,
            });
        }
        let chunks = bytes.len().div_ceil(chunk_bytes);
        if chunks > MAX_CHUNKS {
            return Err(RepeaterOtaError::TooManyChunks {
                chunks,
                limit: MAX_CHUNKS,
            });
        }
        let sha256 = sha256(&bytes);
        Ok(Self {
            bytes,
            chunk_bytes,
            sha256,
        })
    }

    pub fn chunk_count(&self) -> usize {
        self.bytes.len().div_ceil(self.chunk_bytes)
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub fn sha256(&self) -> &[u8; 32] {
        &self.sha256
    }

    pub fn chunk(&self, index: usize) -> Option<&[u8]> {
        let start = index.checked_mul(self.chunk_bytes)?;
        if start >= self.bytes.len() {
            return None;
        }
        let end = (start + self.chunk_bytes).min(self.bytes.len());
        Some(&self.bytes[start..end])
    }

    /// Rough wire time at 115200 8N1, before pacing. Worth surfacing: a full image
    /// is about half a minute per repeater, not a few seconds.
    pub fn estimated_seconds(&self, params: &RepeaterOtaParams) -> f32 {
        let framing = 24.0; // envelope, verb, bin header, CRC, COBS
        let per_chunk = self.chunk_bytes as f32 + framing;
        let bits = per_chunk * 10.0 * self.chunk_count() as f32;
        bits / 115_200.0
            + (self.chunk_count() as f32 * params.wait_between_chunks_ms as f32) / 1000.0
    }
}

fn control_packet(
    target: &RepeaterTarget,
    verb: RepeaterVerb,
    payload: Option<Value>,
    wait_ms: u32,
) -> Packet {
    // Copies the Portal firmware-update packet shape deliberately. `Packet::from_body`
    // would default to `collateable: true`, and the outbox keeps only the newest
    // packet per (address, target) -- which would silently delete every data frame
    // but the last. An empty address never collates.
    // A reply only comes back with a *source* the worker can match, and
    // `reply_source()` has one for `Index` alone. A MAC-addressed repeater answers from
    // -2 when it is unprovisioned and from its own index once it is not, and a broadcast
    // is never answered at all -- so in both cases `Packet.target` would fall back to
    // HOST(0) and `needs_ack` would sit out the entire window waiting for a frame from
    // the host itself. That is 8 s per `ota-begin`, on exactly the commissioning path
    // that has no index yet. The reply is correlated by verb in `await_reply`, which
    // never needed the ack machinery; here it only has to stop costing time.
    let needs_ack = verb.expects_reply() && target.reply_source().is_some();
    // `custom_wait_time_ms` means two different things in the worker: an ack window when
    // `needs_ack`, and a post-send gap when not (`rs485/worker.rs`). Callers pass a window
    // for reply-bearing verbs and a gap for the rest -- so a reply-bearing verb that has
    // just lost its ack must not have its window reinterpreted as an 8-second sleep, and
    // `ota-data`'s pacing gap must survive untouched.
    let wait_ms = if verb.expects_reply() && !needs_ack {
        0
    } else {
        wait_ms
    };
    Packet {
        payload: Payload::Rendered(request(target, verb, payload)),
        target: target.reply_source().unwrap_or(router_proto::HOST),
        address: String::new(),
        needs_ack,
        collateable: false,
        custom_wait_time_ms: Some(wait_ms),
        on_sent: None,
    }
}

fn begin_payload(image: &RepeaterImage, params: &RepeaterOtaParams) -> Value {
    map(vec![
        (key("size"), Value::from(image.len() as u64)),
        (key("chunk"), Value::from(params.chunk_bytes as u64)),
        (key("session"), Value::from(params.session)),
        (key("sha"), Value::Binary(image.sha256().to_vec())),
    ])
}

/// `[session, index, bin(data), crc16]` — an array, not a map, because this is the
/// one verb sent hundreds of times per update.
fn data_payload(params: &RepeaterOtaParams, index: usize, chunk: &[u8]) -> Value {
    Value::Array(vec![
        Value::from(params.session),
        Value::from(index as u64),
        Value::Binary(chunk.to_vec()),
        Value::from(crc16_ccitt_false(chunk)),
    ])
}

/// The acknowledged `ota-begin`. Nothing may be streamed until the repeater has
/// answered: the erase it performs runs with the flash cache disabled, so the UART
/// ISR cannot run and inbound bytes are lost for hundreds of milliseconds.
pub fn begin(
    rs485: &Rs485,
    target: &RepeaterTarget,
    image: &RepeaterImage,
    params: &RepeaterOtaParams,
) -> usize {
    rs485.transmit(control_packet(
        target,
        RepeaterVerb::OtaBegin,
        Some(begin_payload(image, params)),
        params.begin_timeout_ms,
    ));
    1
}

/// Queues the chunks named by `indices`. Used both for the initial pass (every
/// index) and for repair (only the gaps a repeater reported).
///
/// Chunk 0 is always sent first, in every pass, so the receiver's first write to a
/// freshly erased slot is always the one carrying the image header.
pub fn send_chunks(
    rs485: &Rs485,
    target: &RepeaterTarget,
    image: &RepeaterImage,
    params: &RepeaterOtaParams,
    indices: &[usize],
) -> usize {
    let mut ordered: Vec<usize> = indices.to_vec();
    ordered.sort_unstable();
    ordered.dedup();
    if let Some(position) = ordered.iter().position(|index| *index == 0) {
        ordered.remove(position);
        ordered.insert(0, 0);
    }

    let mut queued = 0;
    for index in ordered {
        let Some(chunk) = image.chunk(index) else {
            continue;
        };
        rs485.transmit(control_packet(
            target,
            RepeaterVerb::OtaData,
            Some(data_payload(params, index, chunk)),
            params.wait_between_chunks_ms,
        ));
        queued += 1;
    }
    queued
}

pub fn all_indices(image: &RepeaterImage) -> Vec<usize> {
    (0..image.chunk_count()).collect()
}

/// Asks a repeater which chunks it has. Unicast only.
pub fn request_map(rs485: &Rs485, target: &RepeaterTarget, params: &RepeaterOtaParams) -> usize {
    rs485.transmit(control_packet(
        target,
        RepeaterVerb::OtaMap,
        None,
        params.begin_timeout_ms.min(2000),
    ));
    1
}

/// Turns a received-chunk bitmap into the list of indices still missing.
///
/// The repeater sends the raw bitmap rather than run-lengths: it is a fixed 78
/// bytes for a 617-chunk image, where worst-case run-lengths would be 1.6 kB on a
/// bus where every other repeater has to buffer them.
pub fn missing_from_bitmap(bitmap: &[u8], chunk_count: usize) -> Vec<usize> {
    (0..chunk_count)
        .filter(|index| {
            let byte = index / 8;
            byte >= bitmap.len() || bitmap[byte] & (1 << (index % 8)) == 0
        })
        .collect()
}

/// Verifies and commits. The repeater answers `incomplete` if chunks are still
/// missing, which is a cue to read the bitmap again rather than a failure.
pub fn end(rs485: &Rs485, target: &RepeaterTarget, params: &RepeaterOtaParams) -> usize {
    rs485.transmit(control_packet(
        target,
        RepeaterVerb::OtaEnd,
        None,
        params.end_timeout_ms,
    ));
    1
}

/// Reboots into the newly committed slot. Answers nothing, so it may be broadcast.
pub fn boot(rs485: &Rs485, target: &RepeaterTarget) -> usize {
    rs485.transmit(control_packet(target, RepeaterVerb::OtaBoot, None, 0));
    1
}

/// Confirms a freshly booted image early. Purely an accelerator: a repeater that
/// never hears this still resolves its own pending-verify state within about 30
/// seconds on local evidence, which is what stops a rack that powers up before the
/// show PC from reverting every morning.
pub fn confirm(rs485: &Rs485, target: &RepeaterTarget, params: &RepeaterOtaParams) -> usize {
    rs485.transmit(control_packet(
        target,
        RepeaterVerb::OtaConfirm,
        None,
        params.begin_timeout_ms.min(2000),
    ));
    1
}

/// Abandons a session and lets the repeater resume relaying immediately, rather
/// than waiting out its 30-second inactivity timeout.
pub fn abort(rs485: &Rs485, target: &RepeaterTarget) -> usize {
    rs485.transmit(control_packet(target, RepeaterVerb::OtaAbort, None, 0));
    1
}

pub fn validate_index(index: u8) -> Result<RepeaterTarget, RepeaterOtaError> {
    if (1..=router_proto::REPEATER_COUNT).contains(&index) {
        Ok(RepeaterTarget::Index(index))
    } else {
        Err(RepeaterOtaError::BadIndex(index))
    }
}

/// The control-plane version a repeater must report before the OTA verbs are used.
pub fn required_proto_version() -> u16 {
    CONTROL_PROTO_VERSION
}

/// SHA-256, matching the check the repeater performs on the slot it wrote.
///
/// Public because the images this crate sends are the images a bench has to identify and
/// record, and a second implementation of SHA-256 in the tree would be one too many.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let mut message = data.to_vec();
    let bit_length = (data.len() as u64) * 8;
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_length.to_be_bytes());

    for block in message.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d) = (state[0], state[1], state[2], state[3]);
        let (mut e, mut f, mut g, mut h) = (state[4], state[5], state[6], state[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        for (slot, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }

    let mut digest = [0u8; 32];
    for (i, word) in state.iter().enumerate() {
        digest[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    digest
}

// ---------------------------------------------------------------------------------
// The driver
//
// The steps above are individually harmless; the sequencing between them is not, and
// until now it lived in `examples/flash_repeater.rs` where nothing could call it. Three
// of the invariants below each cost a bench session to find, and a second implementation
// would get at least one of them wrong -- so there is one, and the example drives it.
// ---------------------------------------------------------------------------------

/// How many repair passes are attempted before a transfer is given up on.
pub const MAX_REPAIR_ROUNDS: u8 = 5;

/// Where an update has got to. The fractions the observer is handed are overall, not
/// within-phase, so a caller can put them straight on a progress bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtaPhase {
    Begin,
    Stream,
    Map,
    Repair(u8),
    End,
    Boot,
    Confirm,
}

impl OtaPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            OtaPhase::Begin => "begin",
            OtaPhase::Stream => "stream",
            OtaPhase::Map => "map",
            OtaPhase::Repair(_) => "repair",
            OtaPhase::End => "end",
            OtaPhase::Boot => "boot",
            OtaPhase::Confirm => "confirm",
        }
    }
}

/// What a caller sees while an update runs, and how it stops one.
///
/// The default `cancelled` is `false` so a caller that only wants progress writes one
/// method. Cancellation is checked between chunks and between rounds, never mid-frame:
/// what it abandons is the transfer, and the repeater is always told, so the bridge
/// resumes relaying at once rather than after its 30-second timeout.
pub trait OtaObserver {
    fn phase(&mut self, phase: OtaPhase, fraction: f32, detail: &str);
    fn cancelled(&mut self) -> bool {
        false
    }
}

/// An observer for callers that want neither progress nor cancellation.
pub struct SilentObserver;

impl OtaObserver for SilentObserver {
    fn phase(&mut self, _phase: OtaPhase, _fraction: f32, _detail: &str) {}
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtaTargetReport {
    pub target: RepeaterTarget,
    /// False means the image was committed and booted but `ota-confirm` went unanswered.
    /// **Not a failure**: an unconfirmed image resolves its own pending-verify state in
    /// about 30 seconds on local evidence. Worth reporting, because until then a power
    /// cut rolls the repeater back.
    pub confirmed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OtaReport {
    pub targets: Vec<OtaTargetReport>,
    pub chunks: usize,
    pub repair_rounds: u8,
    pub repaired_chunks: usize,
    pub seconds: f32,
    pub sha256: [u8; 32],
    /// True when the data pass was broadcast because more than one repeater was being
    /// updated at once.
    pub broadcast: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OtaError {
    #[error("repeater {target} did not answer {verb}")]
    NoAnswer { target: String, verb: &'static str },
    #[error("repeater {target} refused {verb}: {detail}")]
    Refused {
        target: String,
        verb: &'static str,
        detail: String,
    },
    #[error("the ota-map reply from repeater {target} carried no bitmap")]
    NoBitmap { target: String },
    #[error("the outbox did not drain; is the port still there?")]
    OutboxStalled,
    #[error(
        "repeater {target} is still missing {missing} of {total} chunks after {rounds} repair rounds"
    )]
    RepairExhausted {
        target: String,
        missing: usize,
        total: usize,
        rounds: u8,
    },
    #[error(
        "repeater {target} reports a complete chunk map and still refuses to commit: its bitmap and its own read-back disagree"
    )]
    BitmapDisagrees { target: String },
    #[error("cancelled")]
    Cancelled,
    #[error("no repeater was named")]
    NoTargets,
    #[error(
        "a broadcast target cannot be updated directly: name the repeaters, and the data pass is broadcast for you"
    )]
    BroadcastTargetNamed,
}

/// How a target reads in a message. `Index(3)` is "3"; a MAC is its usual six pairs.
pub fn describe_target(target: &RepeaterTarget) -> String {
    match target {
        RepeaterTarget::All => "all".to_string(),
        RepeaterTarget::Index(index) => index.to_string(),
        RepeaterTarget::Mac(mac) => mac
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(":"),
    }
}

/// Reads a named field out of a reply payload.
pub fn payload_field<'a>(payload: &'a Option<Value>, name: &str) -> Option<&'a Value> {
    let Value::Map(entries) = payload.as_ref()? else {
        return None;
    };
    entries
        .iter()
        .find(|(k, _)| k.as_str() == Some(name))
        .map(|(_, v)| v)
}

/// Waits for the reply to one verb.
///
/// Matching on the verb matters here: a step that timed out earlier can have its answer
/// still in flight, and accepting it as this step's would report success for work that
/// never happened.
pub fn await_reply(
    rs485: &mut Rs485,
    verb: RepeaterVerb,
    timeout: Duration,
) -> Option<RepeaterReply> {
    let deadline = Instant::now() + timeout;
    loop {
        for envelope in rs485.update() {
            if let Ok(Some(reply)) = parse_reply(&envelope.body) {
                if reply.verb == Some(verb) {
                    return Some(reply);
                }
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// How long a streaming pass really takes.
///
/// The two costs overlap rather than add: the worker's per-packet sleep happens while the
/// OS is still shifting out earlier packets, so a pass takes the longer of "bytes at
/// 115200" and "one gap per packet", not their sum. Adding them overestimates by nearly
/// half, and overestimating is not harmless -- the receiver abandons a session after 30
/// seconds with no accepted chunk, so waiting too long before asking for the map loses a
/// transfer that had in fact completed.
///
/// 10 bits per byte, and 48 bytes of envelope, verb, bin header, CRC and COBS per chunk.
pub fn wire_time(count: usize, params: &RepeaterOtaParams) -> Duration {
    let bytes = count * (params.chunk_bytes + 48);
    let on_the_wire = bytes as f32 * 10.0 / 115_200.0;
    let paced = count as f32 * params.wait_between_chunks_ms as f32 / 1000.0;
    Duration::from_secs_f32(on_the_wire.max(paced) * 1.1)
}

/// Waits until a queued burst is actually on the wire.
///
/// An empty outbox is not enough. `SerialPortDevice::transmit` is a buffered `write_all`,
/// so the worker hands a whole streaming pass to the OS in a couple of seconds while the
/// port is still shifting it out for another half a minute. Asking for the map at that
/// point queues the request behind the backlog and times out against a repeater that is
/// doing exactly what it should.
pub fn drain(
    rs485: &mut Rs485,
    started: Instant,
    wire: Duration,
    timeout: Duration,
    settle: Duration,
) -> bool {
    let deadline = started + timeout;
    while Instant::now() < deadline {
        rs485.update();
        if rs485.outbox_len() == 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    if rs485.outbox_len() != 0 {
        return false;
    }
    // The outbox emptying only means the OS took the bytes. Hold until the wire itself
    // could have carried them, measured from when the burst was queued.
    let until = started + wire + settle;
    while Instant::now() < until {
        rs485.update();
        std::thread::sleep(Duration::from_millis(20));
    }
    true
}

/// Whether the session has reached a point where abandoning it would leave a repeater
/// paused, and `ota-abort` therefore has to be sent.
///
/// This is a flag rather than an RAII guard for a dull reason: the guard would have to
/// hold the bus to abort with, and `Rs485::update` takes `&mut self`, so holding it would
/// lock the driver out of reading its own replies. So the abort lives in [`run_update`],
/// which is a wrapper over [`run_session`] for exactly this purpose -- every `?` inside
/// the session lands there. It is worth the indirection: the failure paths are where an
/// abort matters, and a session abandoned without one leaves that repeater's whole branch
/// dark for the full 30-second inactivity timeout.
type Armed = std::cell::Cell<bool>;

/// Overall progress, so every phase reports on one timeline rather than restarting a bar.
fn overall(phase: OtaPhase, within: f32) -> f32 {
    let (base, span) = match phase {
        OtaPhase::Begin => (0.00, 0.04),
        OtaPhase::Stream => (0.04, 0.68),
        OtaPhase::Map => (0.72, 0.04),
        OtaPhase::Repair(_) => (0.76, 0.12),
        OtaPhase::End => (0.88, 0.05),
        OtaPhase::Boot => (0.93, 0.05),
        OtaPhase::Confirm => (0.98, 0.02),
    };
    (base + span * within.clamp(0.0, 1.0)).clamp(0.0, 1.0)
}

/// Update one repeater, or several at once.
///
/// With one target the data pass is unicast. With several it is broadcast, which is the
/// only part of the sequence that may be: `Protocol.md` §12 makes `ota-begin`, `ota-map`,
/// `ota-end` and `ota-confirm` unicast, and the firmware refuses a reply-bearing verb sent
/// to `REPEATER_ALL`. So a "broadcast update" is N sessions sharing one stream, not one
/// broadcast session -- and it blacks out every branch it touches for the duration, which
/// is why it is something a caller has to ask for by naming more than one repeater.
pub fn run_update(
    rs485: &mut Rs485,
    targets: &[RepeaterTarget],
    image: &RepeaterImage,
    params: &RepeaterOtaParams,
    boot_after: bool,
    observer: &mut dyn OtaObserver,
) -> Result<OtaReport, OtaError> {
    let armed = Armed::new(false);
    let outcome = run_session(rs485, targets, image, params, boot_after, observer, &armed);
    if armed.get() {
        for target in targets {
            abort(rs485, target);
        }
        // The caller is on its way out and the frames are a few dozen bytes each. This is
        // long enough for them to leave, and short enough not to be felt.
        // Long enough for a handful of small frames to leave, and short enough not to be
        // felt. `update` afterwards so the caller's next read is not behind.
        std::thread::sleep(Duration::from_millis(200));
        rs485.update();
    }
    outcome
}

#[allow(clippy::too_many_arguments)]
fn run_session(
    rs485: &mut Rs485,
    targets: &[RepeaterTarget],
    image: &RepeaterImage,
    params: &RepeaterOtaParams,
    boot_after: bool,
    observer: &mut dyn OtaObserver,
    armed: &Armed,
) -> Result<OtaReport, OtaError> {
    if targets.is_empty() {
        return Err(OtaError::NoTargets);
    }
    if targets.iter().any(|t| matches!(t, RepeaterTarget::All)) {
        return Err(OtaError::BroadcastTargetNamed);
    }
    let broadcast = targets.len() > 1;
    let stream_target = if broadcast {
        RepeaterTarget::All
    } else {
        targets[0].clone()
    };
    let started = Instant::now();
    let total = image.chunk_count();

    // ---- begin, unicast, one at a time ------------------------------------------
    // The erase blocks and drops inbound bytes, so nothing may be streamed to any of
    // them until every one has answered.
    for (n, target) in targets.iter().enumerate() {
        check_cancelled(observer)?;
        let name = describe_target(target);
        observer.phase(
            OtaPhase::Begin,
            overall(OtaPhase::Begin, n as f32 / targets.len() as f32),
            &format!("erasing the target slot on repeater {name}"),
        );
        begin(&*rs485, target, image, params);
        // Armed from the first request, not the first success: a `begin` that erased the
        // slot and then lost its reply has still paused that bridge.
        armed.set(true);
        let reply = expect(
            rs485,
            RepeaterVerb::OtaBegin,
            Duration::from_millis(params.begin_timeout_ms as u64),
            &name,
        )?;
        require_ok(&reply, RepeaterVerb::OtaBegin, &name)?;
    }

    // ---- stream once, then repair exactly the gaps -------------------------------
    let mut indices = all_indices(image);
    let mut round: u8 = 0;
    let mut repaired = 0usize;
    let mut resent_everything = false;

    loop {
        let phase = if round == 0 {
            OtaPhase::Stream
        } else {
            OtaPhase::Repair(round)
        };
        let detail = if round == 0 {
            format!("{} chunks", indices.len())
        } else {
            format!("round {round}: {} chunks", indices.len())
        };
        observer.phase(phase, overall(phase, 0.0), &detail);

        let burst_started = Instant::now();
        let queued = send_chunks(&*rs485, &stream_target, image, params, &indices);
        if round > 0 {
            repaired += queued;
        }
        let wire = wire_time(queued, params);
        drain_burst(rs485, burst_started, wire, params, observer, phase, &detail)?;

        // ---- which chunks landed, on every target --------------------------------
        let mut missing: BTreeSet<usize> = BTreeSet::new();
        let mut worst: Option<(String, usize)> = None;
        for target in targets {
            check_cancelled(observer)?;
            let name = describe_target(target);
            observer.phase(
                OtaPhase::Map,
                overall(OtaPhase::Map, 0.5),
                &format!("asking repeater {name} which chunks landed"),
            );
            request_map(&*rs485, target, params);
            let reply = expect(rs485, RepeaterVerb::OtaMap, Duration::from_secs(15), &name)?;
            let Some(Value::Binary(bitmap)) = payload_field(&reply.payload, "map") else {
                return Err(OtaError::NoBitmap { target: name });
            };
            let gaps = missing_from_bitmap(bitmap, total);
            if worst.as_ref().is_none_or(|(_, n)| gaps.len() > *n) {
                worst = Some((name, gaps.len()));
            }
            missing.extend(gaps);
        }

        if missing.is_empty() {
            break;
        }
        round += 1;
        if round > MAX_REPAIR_ROUNDS {
            let (target, count) = worst.unwrap_or_else(|| ("?".to_string(), missing.len()));
            return Err(OtaError::RepairExhausted {
                target,
                missing: count,
                total,
                rounds: MAX_REPAIR_ROUNDS,
            });
        }
        indices = missing.into_iter().collect();
    }

    // ---- commit ------------------------------------------------------------------
    'commit: loop {
        for target in targets {
            check_cancelled(observer)?;
            let name = describe_target(target);
            observer.phase(
                OtaPhase::End,
                overall(OtaPhase::End, 0.5),
                &format!("repeater {name} is hashing the written slot"),
            );
            end(&*rs485, target, params);
            let reply = expect(
                rs485,
                RepeaterVerb::OtaEnd,
                Duration::from_millis(params.end_timeout_ms as u64),
                &name,
            )?;
            if reply.ok {
                continue;
            }
            // "incomplete" is documented as a cue to read the bitmap again rather than a
            // failure -- but we only got here with every bitmap full, so the repeater's
            // map and its own read-back disagree. Re-streaming the whole image is the one
            // recovery that addresses that, and it is worth exactly one attempt: a second
            // identical answer is a fault, not a gap.
            let incomplete = payload_field(&reply.payload, "err")
                .and_then(|v| v.as_str())
                .is_some_and(|s| s.contains("incomplete"));
            if incomplete && !resent_everything {
                resent_everything = true;
                let detail = "re-sending every chunk: the map and the read-back disagree";
                let phase = OtaPhase::Repair(round.saturating_add(1));
                observer.phase(phase, overall(phase, 0.0), detail);
                let burst_started = Instant::now();
                let queued =
                    send_chunks(&*rs485, &stream_target, image, params, &all_indices(image));
                repaired += queued;
                let wire = wire_time(queued, params);
                drain_burst(rs485, burst_started, wire, params, observer, phase, detail)?;
                continue 'commit;
            }
            if incomplete {
                return Err(OtaError::BitmapDisagrees { target: name });
            }
            return Err(refusal(&reply, RepeaterVerb::OtaEnd, &name));
        }
        break;
    }

    // Committed. From here the repeater has a good image in the spare slot, and an abort
    // would only throw away work that succeeded.
    armed.set(false);

    let mut reports: Vec<OtaTargetReport> = targets
        .iter()
        .map(|target| OtaTargetReport {
            target: target.clone(),
            confirmed: false,
        })
        .collect();

    if boot_after {
        observer.phase(
            OtaPhase::Boot,
            overall(OtaPhase::Boot, 0.0),
            "rebooting into the new slot",
        );
        // Give the repeater a breath first. Its reply to `ota-end` ends with its driver
        // letting go of the line, its receiver samples one byte of debris as it comes back,
        // and a frame arriving within its 20 ms incomplete-frame window is glued to that
        // debris and dies -- which is how four consecutive updates on the bench committed
        // and then never rebooted. The firmware now discards debris in its own post-transmit
        // shadow; this pause keeps the sequence working on bridges that predate that fix.
        std::thread::sleep(Duration::from_millis(50));
        boot(&*rs485, &stream_target);
        drain(
            rs485,
            Instant::now(),
            Duration::ZERO,
            Duration::from_secs(3),
            Duration::ZERO,
        );
        // The repeater is not on the bus at all while it restarts, so this wait is real.
        std::thread::sleep(Duration::from_secs(5));
        rs485.update();

        for report in reports.iter_mut() {
            let name = describe_target(&report.target);
            observer.phase(
                OtaPhase::Confirm,
                overall(OtaPhase::Confirm, 0.5),
                &format!("marking repeater {name}'s new image good"),
            );
            confirm(&*rs485, &report.target, params);
            report.confirmed = await_reply(rs485, RepeaterVerb::OtaConfirm, Duration::from_secs(4))
                .is_some_and(|reply| reply.ok);
        }
    }

    observer.phase(OtaPhase::Confirm, 1.0, "done");
    Ok(OtaReport {
        targets: reports,
        chunks: total,
        repair_rounds: round,
        repaired_chunks: repaired,
        seconds: started.elapsed().as_secs_f32(),
        sha256: *image.sha256(),
        broadcast,
    })
}

/// `status`, as a call rather than a packet.
pub fn read_status(
    rs485: &mut Rs485,
    target: &RepeaterTarget,
    timeout: Duration,
) -> Result<RepeaterReply, OtaError> {
    let name = describe_target(target);
    rs485.transmit(control_packet(
        target,
        RepeaterVerb::Status,
        None,
        timeout.as_millis() as u32,
    ));
    expect(rs485, RepeaterVerb::Status, timeout, &name)
}

/// `set-index`, which a repeater answers with its whole status -- so the reply *is* the
/// read-back, and a caller never has to ask twice.
pub fn set_index(
    rs485: &mut Rs485,
    target: &RepeaterTarget,
    index: u8,
    timeout: Duration,
) -> Result<RepeaterReply, OtaError> {
    let name = describe_target(target);
    rs485.transmit(control_packet(
        target,
        RepeaterVerb::SetIndex,
        Some(Value::from(index)),
        timeout.as_millis() as u32,
    ));
    let reply = expect(rs485, RepeaterVerb::SetIndex, timeout, &name)?;
    require_ok(&reply, RepeaterVerb::SetIndex, &name)?;
    Ok(reply)
}

/// `set-polarity`: how one side of a repeater decides its UART polarity. Acknowledged; the
/// caller reads `status` afterwards for the resulting `inv`/`pol`/`lk` per side.
pub fn set_polarity(
    rs485: &mut Rs485,
    target: &RepeaterTarget,
    side: u8,
    mode: router_proto::repeater::PolarityMode,
    timeout: Duration,
) -> Result<RepeaterReply, OtaError> {
    let name = describe_target(target);
    rs485.transmit(control_packet(
        target,
        RepeaterVerb::SetPolarity,
        Some(router_proto::repeater::set_polarity_payload(side, mode)),
        timeout.as_millis() as u32,
    ));
    let reply = expect(rs485, RepeaterVerb::SetPolarity, timeout, &name)?;
    require_ok(&reply, RepeaterVerb::SetPolarity, &name)?;
    Ok(reply)
}

fn check_cancelled(observer: &mut dyn OtaObserver) -> Result<(), OtaError> {
    if observer.cancelled() {
        Err(OtaError::Cancelled)
    } else {
        Ok(())
    }
}

fn expect(
    rs485: &mut Rs485,
    verb: RepeaterVerb,
    timeout: Duration,
    target: &str,
) -> Result<RepeaterReply, OtaError> {
    await_reply(rs485, verb, timeout).ok_or_else(|| OtaError::NoAnswer {
        target: target.to_string(),
        verb: verb.as_str(),
    })
}

fn require_ok(reply: &RepeaterReply, verb: RepeaterVerb, target: &str) -> Result<(), OtaError> {
    if reply.ok {
        Ok(())
    } else {
        Err(refusal(reply, verb, target))
    }
}

fn refusal(reply: &RepeaterReply, verb: RepeaterVerb, target: &str) -> OtaError {
    let detail = payload_field(&reply.payload, "err")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("{:?}", reply.payload));
    OtaError::Refused {
        target: target.to_string(),
        verb: verb.as_str(),
        detail,
    }
}

/// `drain`, with the cancellation check and the progress reporting a long burst needs.
fn drain_burst(
    rs485: &mut Rs485,
    started: Instant,
    wire: Duration,
    params: &RepeaterOtaParams,
    observer: &mut dyn OtaObserver,
    phase: OtaPhase,
    detail: &str,
) -> Result<(), OtaError> {
    let settle = Duration::from_millis(params.settle_after_burst_ms as u64);
    let timeout = wire * 3 + Duration::from_secs(30);
    let deadline = started + timeout;
    let until = started + wire + settle;
    loop {
        rs485.update();
        if observer.cancelled() {
            // Whatever is still queued must not go out: the session is over, and the
            // abort behind it has to reach the repeater ahead of stale chunks.
            rs485.clear_outbox();
            return Err(OtaError::Cancelled);
        }
        let now = Instant::now();
        let drained = rs485.outbox_len() == 0;
        if drained && now >= until {
            return Ok(());
        }
        if !drained && now >= deadline {
            return Err(OtaError::OutboxStalled);
        }
        // Progress is measured against the wire, not the outbox: the outbox empties in a
        // couple of seconds and the port takes half a minute.
        let elapsed = now.saturating_duration_since(started).as_secs_f32();
        let span = (wire + settle).as_secs_f32().max(0.001);
        observer.phase(phase, overall(phase, elapsed / span), detail);
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_known_vectors() {
        assert_eq!(
            hex(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // Straddles the padding boundary, the case that catches length-encoding bugs.
        assert_eq!(
            hex(&sha256(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn an_image_splits_into_chunks_with_a_short_tail() {
        let image = RepeaterImage::new(vec![0xAB; 1000], 512).unwrap();
        assert_eq!(image.chunk_count(), 2);
        assert_eq!(image.chunk(0).unwrap().len(), 512);
        assert_eq!(image.chunk(1).unwrap().len(), 488);
        assert!(image.chunk(2).is_none());
    }

    #[test]
    fn malformed_images_are_refused_before_anything_is_sent() {
        assert!(matches!(
            RepeaterImage::new(vec![], 512),
            Err(RepeaterOtaError::Empty)
        ));
        assert!(matches!(
            RepeaterImage::new(vec![0; 16], 0),
            Err(RepeaterOtaError::BadChunkSize { .. })
        ));
        assert!(matches!(
            RepeaterImage::new(vec![0; 16], MAX_CHUNK_BYTES + 1),
            Err(RepeaterOtaError::BadChunkSize { .. })
        ));
        assert!(matches!(
            RepeaterImage::new(vec![0; APP_SLOT_BYTES + 1], 512),
            Err(RepeaterOtaError::TooLarge { .. })
        ));
        // A one-byte chunk size over a large image exceeds the receiver's bitmap.
        assert!(matches!(
            RepeaterImage::new(vec![0; MAX_CHUNKS + 1], 1),
            Err(RepeaterOtaError::TooManyChunks { .. })
        ));
    }

    #[test]
    fn a_bitmap_names_exactly_the_missing_chunks() {
        // Bits 1 and 6 clear, the pattern a lossy broadcast pass leaves behind.
        assert_eq!(missing_from_bitmap(&[0b1011_1101], 8), vec![1, 6]);
        assert_eq!(missing_from_bitmap(&[0xFF], 8), Vec::<usize>::new());
        assert_eq!(missing_from_bitmap(&[0x00], 3), vec![0, 1, 2]);
        // A bitmap shorter than the image means everything past it is missing,
        // rather than silently assumed present.
        assert_eq!(missing_from_bitmap(&[0xFF], 10), vec![8, 9]);
        assert_eq!(missing_from_bitmap(&[], 2), vec![0, 1]);
    }

    #[test]
    fn a_data_chunk_carries_its_session_index_and_crc() {
        let image = RepeaterImage::new(vec![0x5A; 600], 512).unwrap();
        let params = RepeaterOtaParams {
            session: 42,
            ..Default::default()
        };
        let chunk = image.chunk(1).unwrap();
        let Value::Array(fields) = data_payload(&params, 1, chunk) else {
            panic!("payload is not an array");
        };
        assert_eq!(fields[0].as_u64(), Some(42));
        assert_eq!(fields[1].as_u64(), Some(1));
        assert_eq!(fields[2].as_slice().unwrap().len(), 88);
        assert_eq!(fields[3].as_u64(), Some(crc16_ccitt_false(chunk) as u64));
    }

    #[test]
    fn every_pass_puts_chunk_zero_first() {
        // The receiver's first write into a freshly erased slot should always be
        // the one carrying the image header, including on a repair pass.
        let mut indices = vec![7, 3, 0, 5];
        indices.sort_unstable();
        indices.dedup();
        if let Some(position) = indices.iter().position(|i| *i == 0) {
            indices.remove(position);
            indices.insert(0, 0);
        }
        assert_eq!(indices, vec![0, 3, 5, 7]);
    }

    #[test]
    fn timing_estimate_is_the_honest_order_of_magnitude() {
        // The real repeater image, at the default chunk size.
        let image = RepeaterImage::new(vec![0; 315_904], 512).unwrap();
        let seconds = image.estimated_seconds(&RepeaterOtaParams::default());
        // Tens of seconds per repeater, not a few. Anyone planning a maintenance
        // window needs this number to be right.
        assert!(seconds > 25.0, "{seconds} s");
        assert!(seconds < 60.0, "{seconds} s");
    }
}

/// The sequencing, against a bus that speaks the real wire format.
///
/// Deliberately not a mock of `Rs485`: the frames go through real COBS framing, a real
/// MessagePack envelope, the real `parse_reply`, the real bitmap arithmetic and the real
/// repair loop. A harness that simulated the decoder would prove nothing about the
/// decoder — which is exactly what the repair loop most needs proving about.
#[cfg(test)]
mod session_tests {
    use super::*;
    use crate::rs485::device::SerialDevice;
    use router_proto::envelope::encode_reply_fix8;
    use router_proto::repeater::{repeater_address, REPEATER_ALL};
    use router_proto::{decode_envelope, encode_frame, FrameAccumulator};
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    /// What one modelled repeater does with what it is sent.
    #[derive(Default, Clone)]
    struct Behaviour {
        /// Chunks refused the first time they arrive and accepted afterwards: a lossy
        /// wire that the repair loop is supposed to close.
        drop_once: BTreeSet<usize>,
        /// Chunks never accepted at all: a fault the repair loop must give up on.
        never_accept: BTreeSet<usize>,
        silent_on_begin: bool,
        end_refuses: bool,
        end_incomplete_once: bool,
    }

    #[derive(Default)]
    struct Bus {
        behaviour: Behaviour,
        indices: Vec<u8>,
        chunk_total: usize,
        received: BTreeSet<usize>,
        seen: BTreeMap<usize, usize>,
        /// Every verb the driver sent, in order, with the address it was sent to.
        log: Vec<(String, i8)>,
        outbox: Vec<u8>,
        accumulator: FrameAccumulator,
        end_calls: usize,
        connected: bool,
    }

    #[derive(Clone)]
    struct ScriptedBus(Arc<Mutex<Bus>>);

    impl ScriptedBus {
        fn new(indices: &[u8], chunk_total: usize, behaviour: Behaviour) -> Self {
            Self(Arc::new(Mutex::new(Bus {
                behaviour,
                indices: indices.to_vec(),
                chunk_total,
                connected: true,
                ..Bus::default()
            })))
        }

        fn verbs(&self) -> Vec<String> {
            self.0
                .lock()
                .unwrap()
                .log
                .iter()
                .map(|(verb, _)| verb.clone())
                .collect()
        }

        /// Which chunk indices were streamed, in the order they went out, per pass.
        fn passes(&self) -> Vec<Vec<usize>> {
            let bus = self.0.lock().unwrap();
            let mut passes: Vec<Vec<usize>> = Vec::new();
            let mut current: Vec<usize> = Vec::new();
            for (verb, _) in &bus.log {
                if let Some(rest) = verb.strip_prefix("ota-data:") {
                    current.push(rest.parse().unwrap());
                } else if !current.is_empty() {
                    passes.push(std::mem::take(&mut current));
                }
            }
            if !current.is_empty() {
                passes.push(current);
            }
            passes
        }
    }

    impl SerialDevice for ScriptedBus {
        fn type_name(&self) -> &'static str {
            "scripted"
        }
        fn address_string(&self) -> String {
            "scripted".into()
        }
        fn is_connected(&self) -> bool {
            self.0.lock().unwrap().connected
        }
        fn close(&mut self) {
            self.0.lock().unwrap().connected = false;
        }

        fn transmit(&mut self, data: &[u8]) -> std::io::Result<()> {
            let mut bus = self.0.lock().unwrap();
            let frames = bus.accumulator.push(data);
            for frame in frames.into_iter().flatten() {
                let Ok(envelope) = decode_envelope(&frame) else {
                    continue;
                };
                bus.handle(&envelope.body);
            }
            Ok(())
        }

        fn receive_available(&mut self) -> std::io::Result<Vec<u8>> {
            Ok(std::mem::take(&mut self.0.lock().unwrap().outbox))
        }
    }

    impl Bus {
        /// The addresses this bus answers for, and whether `a` names them.
        fn addressed(&self, a: i8) -> Vec<u8> {
            if a == REPEATER_ALL {
                self.indices.clone()
            } else {
                self.indices
                    .iter()
                    .copied()
                    .filter(|index| repeater_address(*index) == Some(a))
                    .collect()
            }
        }

        fn handle(&mut self, body: &Value) {
            let Some(rq) = field(body, "rq") else { return };
            let Some(a) = field(rq, "a").and_then(|v| v.as_i64()) else {
                return;
            };
            let Some(verb) = field(rq, "q").and_then(|v| v.as_str()).map(str::to_string) else {
                return;
            };
            let payload = field(rq, "v").cloned();
            let a = a as i8;
            let targets = self.addressed(a);

            if verb == "ota-data" {
                let Some(Value::Array(items)) = &payload else {
                    return;
                };
                let index = items[1].as_u64().unwrap() as usize;
                self.log.push((format!("ota-data:{index}"), a));
                let times = self.seen.entry(index).or_insert(0);
                *times += 1;
                let refuse = self.behaviour.never_accept.contains(&index)
                    || (self.behaviour.drop_once.contains(&index) && *times == 1);
                if !refuse {
                    self.received.insert(index);
                }
                return;
            }

            self.log.push((verb.clone(), a));
            for index in targets {
                match verb.as_str() {
                    "ota-begin" => {
                        if self.behaviour.silent_on_begin {
                            continue;
                        }
                        self.received.clear();
                        self.seen.clear();
                        self.reply(index, "ota-begin", true, None);
                    }
                    "ota-map" => {
                        let mut bitmap = vec![0u8; self.chunk_total.div_ceil(8)];
                        for got in &self.received {
                            bitmap[got / 8] |= 1 << (got % 8);
                        }
                        let got = self.received.len() as u64;
                        self.reply(
                            index,
                            "ota-map",
                            true,
                            Some(map(vec![
                                (key("map"), Value::Binary(bitmap)),
                                (key("got"), Value::from(got)),
                            ])),
                        );
                    }
                    "ota-end" => {
                        self.end_calls += 1;
                        if self.behaviour.end_refuses {
                            self.reply(
                                index,
                                "ota-end",
                                false,
                                Some(map(vec![(key("err"), Value::from("sha-mismatch"))])),
                            );
                        } else if self.behaviour.end_incomplete_once && self.end_calls == 1 {
                            self.reply(
                                index,
                                "ota-end",
                                false,
                                Some(map(vec![(key("err"), Value::from("incomplete"))])),
                            );
                        } else {
                            self.reply(index, "ota-end", true, None);
                        }
                    }
                    "ota-confirm" => self.reply(index, "ota-confirm", true, None),
                    "status" => self.reply(
                        index,
                        "status",
                        true,
                        Some(map(vec![(key("proto"), Value::from(1u16))])),
                    ),
                    "set-index" => self.reply(
                        index,
                        "set-index",
                        true,
                        Some(map(vec![(key("idx"), Value::from(3u8))])),
                    ),
                    // ota-boot and ota-abort answer nothing, by design.
                    _ => {}
                }
            }
        }

        fn reply(&mut self, index: u8, verb: &str, ok: bool, payload: Option<Value>) {
            let source = repeater_address(index).unwrap();
            let mut fields = vec![
                (key("a"), Value::from(source)),
                (key("q"), Value::from(verb)),
                (key("ok"), Value::from(ok)),
            ];
            if let Some(payload) = payload {
                fields.push((key("v"), payload));
            }
            let body = map(vec![(key("rr"), map(fields))]);
            // `[0, source, body]`, the device-to-host direction. Encoding a reply with
            // `encode_envelope` instead puts the address in the *target* slot and leaves
            // the source at 0 -- which `parse_reply` does not notice, because it reads the
            // address out of the `rr` body, but the RS485 worker's ack watch does: it
            // matches on the envelope source, so every acked packet would sit out its full
            // window against a reply that had already arrived.
            self.outbox
                .extend(encode_frame(&encode_reply_fix8(source, &body)));
        }
    }

    fn field<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
        let Value::Map(entries) = value else {
            return None;
        };
        entries
            .iter()
            .find(|(k, _)| k.as_str() == Some(name))
            .map(|(_, v)| v)
    }

    /// Eight chunks of 64 bytes: enough for indices to be meaningful, small enough that
    /// the whole suite runs in well under a second.
    fn fixture(
        behaviour: Behaviour,
        indices: &[u8],
    ) -> (Rs485, ScriptedBus, RepeaterImage, RepeaterOtaParams) {
        let params = RepeaterOtaParams {
            chunk_bytes: 64,
            wait_between_chunks_ms: 0,
            begin_timeout_ms: 1000,
            end_timeout_ms: 1000,
            settle_after_burst_ms: 0,
            session: 1,
        };
        let bytes: Vec<u8> = (0..64 * 8).map(|i| (i % 251) as u8).collect();
        let image = RepeaterImage::new(bytes, params.chunk_bytes).unwrap();
        let bus = ScriptedBus::new(indices, image.chunk_count(), behaviour);
        let mut rs485 = Rs485::new(0, router_report::Reporter::disabled());
        rs485.open_device(Box::new(bus.clone()));
        rs485.update();
        (rs485, bus, image, params)
    }

    fn targets(indices: &[u8]) -> Vec<RepeaterTarget> {
        indices.iter().map(|i| RepeaterTarget::Index(*i)).collect()
    }

    #[test]
    fn a_clean_transfer_needs_no_repair() {
        let (mut rs485, bus, image, params) = fixture(Behaviour::default(), &[1]);
        let report = run_update(
            &mut rs485,
            &targets(&[1]),
            &image,
            &params,
            false,
            &mut SilentObserver,
        )
        .expect("clean transfer");
        assert_eq!(report.repair_rounds, 0);
        assert_eq!(report.repaired_chunks, 0);
        assert_eq!(report.chunks, 8);
        assert!(!report.broadcast);
        let verbs = bus.verbs();
        assert_eq!(verbs.first().map(String::as_str), Some("ota-begin"));
        assert!(verbs.contains(&"ota-end".to_string()));
        // Nothing was booted, so nothing was confirmed and nothing was aborted.
        assert!(!verbs.contains(&"ota-boot".to_string()));
        assert!(!verbs.contains(&"ota-abort".to_string()));
    }

    #[test]
    fn the_repair_pass_re_sends_exactly_the_gaps_with_chunk_zero_first() {
        let behaviour = Behaviour {
            drop_once: [1usize, 6].into_iter().collect(),
            ..Behaviour::default()
        };
        let (mut rs485, bus, image, params) = fixture(behaviour, &[1]);
        let report = run_update(
            &mut rs485,
            &targets(&[1]),
            &image,
            &params,
            false,
            &mut SilentObserver,
        )
        .expect("repaired transfer");
        assert_eq!(report.repair_rounds, 1);
        let passes = bus.passes();
        assert_eq!(passes.len(), 2, "one stream and one repair: {passes:?}");
        // The first pass carries the whole image, chunk 0 first: the receiver's first
        // write into a freshly erased slot has to be the one with the image header.
        assert_eq!(passes[0], (0..8).collect::<Vec<_>>());
        // A repair pass carries *exactly* the gaps and nothing else. Chunk 0 already
        // landed, so re-sending it would be work for nothing.
        assert_eq!(passes[1], vec![1, 6]);
        assert_eq!(report.repaired_chunks, 2);
    }

    #[test]
    fn a_silent_begin_streams_nothing_and_aborts() {
        let behaviour = Behaviour {
            silent_on_begin: true,
            ..Behaviour::default()
        };
        let (mut rs485, bus, image, params) = fixture(behaviour, &[1]);
        let error = run_update(
            &mut rs485,
            &targets(&[1]),
            &image,
            &params,
            false,
            &mut SilentObserver,
        )
        .expect_err("no answer to ota-begin");
        assert!(
            matches!(&error, OtaError::NoAnswer { verb, .. } if *verb == "ota-begin"),
            "{error}"
        );
        // The erase may well have happened, so the bridge is paused and must be told.
        assert!(bus.verbs().contains(&"ota-abort".to_string()));
        // And not one byte of the image was streamed at a receiver that could not take it.
        assert!(bus.passes().is_empty());
    }

    #[test]
    fn a_refused_end_does_not_boot_and_does_abort() {
        let behaviour = Behaviour {
            end_refuses: true,
            ..Behaviour::default()
        };
        let (mut rs485, bus, image, params) = fixture(behaviour, &[1]);
        let error = run_update(
            &mut rs485,
            &targets(&[1]),
            &image,
            &params,
            true,
            &mut SilentObserver,
        )
        .expect_err("refused ota-end");
        assert!(
            matches!(&error, OtaError::Refused { verb, detail, .. }
                if *verb == "ota-end" && detail == "sha-mismatch"),
            "{error}"
        );
        let verbs = bus.verbs();
        assert!(!verbs.contains(&"ota-boot".to_string()), "{verbs:?}");
        assert!(verbs.contains(&"ota-abort".to_string()), "{verbs:?}");
    }

    #[test]
    fn an_incomplete_end_re_streams_everything_once() {
        let behaviour = Behaviour {
            end_incomplete_once: true,
            ..Behaviour::default()
        };
        let (mut rs485, bus, image, params) = fixture(behaviour, &[1]);
        let report = run_update(
            &mut rs485,
            &targets(&[1]),
            &image,
            &params,
            false,
            &mut SilentObserver,
        )
        .expect("recovered from a stale bitmap");
        let passes = bus.passes();
        assert_eq!(passes.len(), 2, "{passes:?}");
        assert_eq!(passes[1], (0..8).collect::<Vec<_>>());
        assert_eq!(report.repaired_chunks, 8);
    }

    #[test]
    fn a_chunk_that_never_lands_gives_up_after_five_rounds() {
        let behaviour = Behaviour {
            never_accept: [4usize].into_iter().collect(),
            ..Behaviour::default()
        };
        let (mut rs485, bus, image, params) = fixture(behaviour, &[1]);
        let error = run_update(
            &mut rs485,
            &targets(&[1]),
            &image,
            &params,
            false,
            &mut SilentObserver,
        )
        .expect_err("one chunk never lands");
        assert!(
            matches!(&error, OtaError::RepairExhausted { missing, rounds, total, .. }
                if *missing == 1 && *rounds == MAX_REPAIR_ROUNDS && *total == 8),
            "{error}"
        );
        // One stream plus MAX_REPAIR_ROUNDS repairs, and no more.
        assert_eq!(bus.passes().len(), 1 + MAX_REPAIR_ROUNDS as usize);
        assert!(bus.verbs().contains(&"ota-abort".to_string()));
    }

    #[test]
    fn cancelling_mid_transfer_aborts_the_session() {
        struct CancelAfterBegin(u32);
        impl OtaObserver for CancelAfterBegin {
            fn phase(&mut self, _phase: OtaPhase, _fraction: f32, _detail: &str) {}
            fn cancelled(&mut self) -> bool {
                self.0 += 1;
                self.0 > 2
            }
        }
        let (mut rs485, bus, image, params) = fixture(Behaviour::default(), &[1]);
        let error = run_update(
            &mut rs485,
            &targets(&[1]),
            &image,
            &params,
            false,
            &mut CancelAfterBegin(0),
        )
        .expect_err("cancelled");
        assert!(matches!(error, OtaError::Cancelled), "{error}");
        // The whole point: a cancelled session releases the bridge at once rather than
        // leaving nine Portals dark for the 30-second inactivity timeout.
        assert!(bus.verbs().contains(&"ota-abort".to_string()));
    }

    #[test]
    fn two_repeaters_share_one_broadcast_stream_and_keep_unicast_control() {
        let (mut rs485, bus, image, params) = fixture(Behaviour::default(), &[1, 2]);
        let report = run_update(
            &mut rs485,
            &targets(&[1, 2]),
            &image,
            &params,
            false,
            &mut SilentObserver,
        )
        .expect("broadcast transfer");
        assert!(report.broadcast);
        let log = bus.0.lock().unwrap().log.clone();
        for (verb, address) in &log {
            if verb.starts_with("ota-data:") {
                assert_eq!(
                    *address, REPEATER_ALL,
                    "the data pass is the broadcast half"
                );
            } else {
                // Protocol.md 12: the firmware refuses a reply-bearing verb sent to
                // REPEATER_ALL, so every one of these has to be unicast even here.
                assert_ne!(*address, REPEATER_ALL, "{verb} must stay unicast");
            }
        }
        assert_eq!(
            log.iter().filter(|(v, _)| v == "ota-begin").count(),
            2,
            "one begin per repeater"
        );
        assert_eq!(
            bus.passes().len(),
            1,
            "and only one stream between the two of them"
        );
    }

    #[test]
    fn a_broadcast_target_cannot_be_named_directly() {
        let (mut rs485, _bus, image, params) = fixture(Behaviour::default(), &[1]);
        let error = run_update(
            &mut rs485,
            &[RepeaterTarget::All],
            &image,
            &params,
            false,
            &mut SilentObserver,
        )
        .expect_err("All is derived, never requested");
        assert!(matches!(error, OtaError::BroadcastTargetNamed), "{error}");
    }

    #[test]
    fn a_mac_addressed_request_does_not_wait_for_an_ack_that_cannot_come() {
        // A MAC-addressed repeater answers from -2 or from its own index, never from
        // HOST -- so an acked packet would sit out the whole window every time, on
        // exactly the commissioning path that has no index yet.
        let packet = control_packet(
            &RepeaterTarget::Mac([0xf8, 0x5b, 0x1b, 0xed, 0x8d, 0xa4]),
            RepeaterVerb::OtaBegin,
            None,
            8000,
        );
        assert!(!packet.needs_ack);
        assert_eq!(packet.custom_wait_time_ms, Some(0));

        // An indexed one still does, because its reply has a source the worker can match.
        let packet = control_packet(
            &RepeaterTarget::Index(1),
            RepeaterVerb::OtaBegin,
            None,
            8000,
        );
        assert!(packet.needs_ack);
        assert_eq!(packet.custom_wait_time_ms, Some(8000));

        // And the streaming gap survives both: `ota-data` is never acked, so its wait is
        // the pacing gap and must not be reset to zero.
        let packet = control_packet(&RepeaterTarget::All, RepeaterVerb::OtaData, None, 2);
        assert!(!packet.needs_ack);
        assert_eq!(packet.custom_wait_time_ms, Some(2));
    }

    #[test]
    fn the_two_time_estimates_agree() {
        // `wire_time` paces the driver and `estimated_seconds` is what an operator is
        // told to expect. They are two estimates of one thing, in two functions, and they
        // have drifted before.
        let params = RepeaterOtaParams::default();
        let image = RepeaterImage::new(vec![0u8; 353_456], params.chunk_bytes).unwrap();
        let paced = wire_time(image.chunk_count(), &params).as_secs_f32() / 1.1;
        let quoted = image.estimated_seconds(&params);
        let ratio = paced / quoted;
        assert!(
            (0.8..=1.25).contains(&ratio),
            "wire_time says {paced:.1}s and estimated_seconds says {quoted:.1}s"
        );
    }

    #[test]
    fn progress_is_monotonic_across_every_phase() {
        let phases = [
            OtaPhase::Begin,
            OtaPhase::Stream,
            OtaPhase::Map,
            OtaPhase::Repair(1),
            OtaPhase::End,
            OtaPhase::Boot,
            OtaPhase::Confirm,
        ];
        let mut previous = -1.0f32;
        for phase in phases {
            for step in 0..=10 {
                let value = overall(phase, step as f32 / 10.0);
                // The tolerance is for the join between two bands: `0.04 + 0.68` is not
                // exactly `0.72` in f32, and a bar that moves back by 5e-8 is not the
                // thing this test is about.
                assert!(
                    value >= previous - 1e-6,
                    "{phase:?} at {step} went backwards: {value} < {previous}"
                );
                previous = previous.max(value);
            }
        }
        assert!(previous <= 1.0);
    }
}
