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

use router_proto::repeater::{
    crc16_ccitt_false, request, RepeaterTarget, RepeaterVerb, CONTROL_PROTO_VERSION,
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
}

impl Default for RepeaterOtaParams {
    fn default() -> Self {
        Self {
            chunk_bytes: 512,
            wait_between_chunks_ms: 2,
            begin_timeout_ms: 8000,
            end_timeout_ms: 8000,
            session: 1,
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
        bits / 115_200.0 + (self.chunk_count() as f32 * params.wait_between_chunks_ms as f32) / 1000.0
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
    let needs_ack = verb.expects_reply();
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
fn sha256(data: &[u8]) -> [u8; 32] {
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
            hex(&sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")),
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
