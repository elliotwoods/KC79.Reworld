//! The message envelope: `[target, source, body]`, optionally `[target, source, body, seq, crc16]`.
//!
//! # The trailer
//!
//! PortalFW sends every reply as a 5-element array whose last two elements are a sequence number
//! and a CRC-16 over everything before them (`RS485::finishFrame`,
//! `PortalFW/src/Modules/RS485.cpp`). The extra elements are invisible to any older reader,
//! because every decoder in this project requires only `size() >= 3` and ignores the rest -- which
//! is what allows a hardened frame and a legacy frame to share a bus.
//!
//! Two properties make the trailer parseable without ambiguity, and both are contracts rather than
//! conveniences:
//!
//! - `seq` is always a forced `uint8` (`0xCC` + 1 byte) and `crc16` always a forced `uint16`
//!   (`0xCD` + 2 big-endian bytes), never minimised to a fixint. So the trailer is *always* the
//!   last 5 bytes, and a receiver never has to re-parse the body to find where it starts.
//! - The CRC covers every decoded byte from the array header up to **and including** the `seq`
//!   field, i.e. everything except the final 3 bytes. That is exactly what the firmware's running
//!   CRC contains at the moment it snapshots (`COBSRWStream::checkChecksum` reads `seq`, then
//!   snapshots, then reads the CRC, so the CRC field cannot fold into its own check).

use rmpv::Value;

use crate::crc::crc16_ccitt_false;
use crate::error::ProtoError;
use crate::value::{dump, dump_int, dump_to_vec, write_fix_int8};

/// Broadcast address (all portals on the bus).
pub const BROADCAST: i8 = -1;
/// Host (router) address.
pub const HOST: i8 = 0;

/// Bytes occupied by `seq` (`0xCC` + value) and `crc16` (`0xCD` + 2 bytes).
const TRAILER_BYTES: usize = 5;
/// Bytes occupied by the `crc16` field alone -- the part the CRC does not cover.
const CRC_FIELD_BYTES: usize = 3;

#[derive(Debug, Clone, PartialEq)]
pub struct Envelope {
    pub target: i8,
    pub source: i8,
    pub body: Value,
    /// Whether this frame carried a verified `[seq, crc16]` trailer.
    pub trailer: Trailer,
}

/// The outcome of looking for a trailer on a decoded frame.
///
/// [`Trailer::Absent`] is not an error: a 3-element frame is a legacy frame, and the fielded
/// bootloader and older applications send nothing else. Only [`Trailer::Bad`] means "this frame
/// was corrupted in flight", and only that should cause a receiver to drop it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trailer {
    /// No trailer: fewer than 5 elements, or the last two are not the forced-width pair.
    Absent,
    /// A trailer whose CRC verified.
    Ok { seq: u8 },
    /// A trailer whose CRC did not match. The frame must be discarded.
    Bad { expected: u16, found: u16 },
}

impl Trailer {
    /// Whether a receiver should act on this frame. A missing trailer is acceptable (legacy);
    /// a broken one never is.
    pub fn acceptable(self) -> bool {
        !matches!(self, Trailer::Bad { .. })
    }

    pub fn seq(self) -> Option<u8> {
        match self {
            Trailer::Ok { seq } => Some(seq),
            _ => None,
        }
    }
}

/// Append `[seq, crc16]` to an already-serialised array body and return the finished frame.
///
/// The CRC is computed over `out` as it stands *after* the seq field is written, which is why this
/// cannot be expressed as "encode the two values and append": the second value's input is the
/// encoding of the first.
fn push_trailer(out: &mut Vec<u8>, seq: u8) {
    out.push(0xCC);
    out.push(seq);
    let crc = crc16_ccitt_false(out);
    out.push(0xCD);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// Encode `[target, 0, body, seq, crc16]` with minimal int addresses.
///
/// The shape every host-to-bootloader control-plane request uses.
pub fn encode_envelope_trailer(target: i8, body: &Value, seq: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(24);
    out.push(0x95); // fixarray(5)
    dump_int(i64::from(target), &mut out);
    dump_int(i64::from(HOST), &mut out);
    dump(body, &mut out);
    push_trailer(&mut out, seq);
    out
}

/// Encode `[target, 0, body, seq, crc16]` with forced-int8 addresses and an already-serialised
/// body -- the firmware-update path's header style, for data frames that now carry a trailer.
pub fn encode_envelope_fix8_trailer(target: i8, body_msgpack: &[u8], seq: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(10 + body_msgpack.len());
    out.push(0x95);
    write_fix_int8(target, &mut out);
    write_fix_int8(HOST, &mut out);
    out.extend_from_slice(body_msgpack);
    push_trailer(&mut out, seq);
    out
}

/// Encode a device -> host reply `[0, source, body, seq, crc16]`, the way PortalFW and the v6
/// bootloader do. Used by the firmware simulator and by tests that need a realistic reply.
pub fn encode_reply_trailer(source: i8, body: &Value, seq: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(24);
    out.push(0x95);
    write_fix_int8(HOST, &mut out);
    write_fix_int8(source, &mut out);
    dump(body, &mut out);
    push_trailer(&mut out, seq);
    out
}

/// Declared element count of a msgpack array, from its header alone.
fn array_len(msgpack: &[u8]) -> Option<usize> {
    match *msgpack.first()? {
        header @ 0x90..=0x9F => Some(usize::from(header & 0x0F)),
        0xDC => msgpack
            .get(1..3)
            .map(|b| usize::from(u16::from_be_bytes([b[0], b[1]]))),
        0xDD => msgpack
            .get(1..5)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize),
        _ => None,
    }
}

/// Look for a `[seq, crc16]` trailer on a decoded (un-COBSed) frame and verify it.
///
/// Recognises a trailer only in its forced-width form, and only when the array header actually
/// declares five or more elements -- so a body that merely happens to end in bytes resembling a
/// trailer cannot be mistaken for one.
pub fn check_trailer(msgpack: &[u8]) -> Trailer {
    if msgpack.len() < TRAILER_BYTES + 1 {
        return Trailer::Absent;
    }
    if array_len(msgpack).is_none_or(|len| len < 5) {
        return Trailer::Absent;
    }
    let tail = msgpack.len() - TRAILER_BYTES;
    if msgpack[tail] != 0xCC || msgpack[tail + 2] != 0xCD {
        return Trailer::Absent;
    }
    let covered = msgpack.len() - CRC_FIELD_BYTES;
    let found = u16::from_be_bytes([msgpack[covered + 1], msgpack[covered + 2]]);
    let expected = crc16_ccitt_false(&msgpack[..covered]);
    if expected == found {
        Trailer::Ok { seq: msgpack[tail + 1] }
    } else {
        Trailer::Bad { expected, found }
    }
}

/// Encode an envelope the way `Portal::sendToPortal` / `Column::broadcast`
/// do (msgpack11): `[target, 0, body]` with minimal int encoding, so a
/// portal target of 8 is a positive fixint and broadcast -1 is `0xFF`.
pub fn encode_envelope(target: i8, body: &Value) -> Vec<u8> {
    let mut out = Vec::with_capacity(16);
    out.push(0x93); // fixarray(3)
    dump_int(target as i64, &mut out);
    dump_int(0, &mut out);
    dump(body, &mut out);
    out
}

/// Encode an envelope the way `RS485::makeHeader` / `FWUpdate` do
/// (msgpack-c `msgpack_pack_fix_int8`): forced `0xD0` int8 addresses.
/// `body_msgpack` is appended verbatim (already-serialized body).
pub fn encode_envelope_fix8(target: i8, body_msgpack: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + body_msgpack.len());
    out.push(0x93);
    write_fix_int8(target, &mut out);
    write_fix_int8(0, &mut out);
    out.extend_from_slice(body_msgpack);
    out
}

/// Serialize a body value and wrap it in a fix8 header.
pub fn encode_envelope_fix8_value(target: i8, body: &Value) -> Vec<u8> {
    encode_envelope_fix8(target, &dump_to_vec(body))
}

/// Encode a device -> host reply `[0, source, body]` with the forced-int8
/// header the firmware uses (msgpack-arduino `writeInt8`). Used by the
/// firmware simulator.
pub fn encode_reply_fix8(source: i8, body: &Value) -> Vec<u8> {
    let mut out = Vec::with_capacity(16);
    out.push(0x93);
    write_fix_int8(HOST, &mut out);
    write_fix_int8(source, &mut out);
    dump(body, &mut out);
    out
}

/// Decode an envelope, tolerant of any msgpack int widths (the firmware
/// uses `0xD0`, msgpack11 uses fixints). Arrays longer than 3 elements are
/// accepted (extra elements ignored), matching `Column::processIncoming`
/// which only requires `size() >= 3`.
///
/// A `[seq, crc16]` trailer, if present, is verified and reported in [`Envelope::trailer`]. It is
/// deliberately *reported* rather than enforced: a corrupt frame still decodes to plausible
/// addresses and a plausible body, and the decision to act on it belongs to the caller, which
/// knows whether it is talking to hardware that emits trailers at all.
pub fn decode_envelope(msgpack: &[u8]) -> Result<Envelope, ProtoError> {
    let mut cursor = msgpack;
    let value = rmpv::decode::read_value(&mut cursor)
        .map_err(|e| ProtoError::Msgpack(e.to_string()))?;
    let Value::Array(mut items) = value else {
        return Err(ProtoError::BadEnvelope);
    };
    if items.len() < 3 {
        return Err(ProtoError::BadEnvelope);
    }
    let body = items.remove(2);
    let source = as_i8(&items[1]).ok_or(ProtoError::BadAddress)?;
    let target = as_i8(&items[0]).ok_or(ProtoError::BadAddress)?;
    Ok(Envelope {
        target,
        source,
        body,
        trailer: check_trailer(msgpack),
    })
}

fn as_i8(v: &Value) -> Option<i8> {
    v.as_i64().and_then(|i| i8::try_from(i).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cobs::{cobs_decode, encode_frame};
    use crate::value::key;

    fn hex_to_bytes(s: &str) -> Vec<u8> {
        s.split_whitespace()
            .map(|b| u8::from_str_radix(b, 16).unwrap())
            .collect()
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
    }

    /// The captured wire frame from `IPython/2024-11-23 - COBS issues/
    /// cobsissue.py`: a firmware position report `[0, 1, {"p": [94848, 0,
    /// 94848, 0]}]` (94848 was MICROSTEPS_PER_PRISM_ROTATION / 2 at the time
    /// of capture; the constant is 189_704 -> 94_852 since the 2026 rounding
    /// fix, but the historical bytes are what this test decodes).
    #[test]
    fn golden_captured_frame_decodes() {
        let wire = hex_to_bytes(
            "03 93 D0 08 D0 01 81 A1 70 94 D2 05 01 72 80 D2 01 01 01 02 D2 05 01 72 80 D2 01 01 01 01 00",
        );
        // Strip the trailing delimiter, COBS-decode, envelope-decode
        let payload = cobs_decode(&wire[..wire.len() - 1]).unwrap();
        let envelope = decode_envelope(&payload).unwrap();
        assert_eq!(envelope.target, 0, "addressed to host");
        assert_eq!(envelope.source, 1, "from portal 1");
        let Value::Map(entries) = &envelope.body else { panic!("body not a map") };
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0.as_str(), Some("p"));
        let Value::Array(p) = &entries[0].1 else { panic!("p not an array") };
        let values: Vec<i64> = p.iter().map(|v| v.as_i64().unwrap()).collect();
        assert_eq!(values, vec![94_848, 0, 94_848, 0]);
    }

    #[test]
    fn broadcast_poll_frame_bytes() {
        // Column::broadcast({"poll": nil}) -> [-1, 0, {"poll": nil}] via msgpack11
        let body = Value::Map(vec![(key("poll"), Value::Nil)]);
        let msgpack = encode_envelope(BROADCAST, &body);
        assert_eq!(hex(&msgpack), "93 FF 00 81 A4 70 6F 6C 6C C0");
        let framed = encode_frame(&msgpack);
        assert_eq!(hex(&framed), "03 93 FF 08 81 A4 70 6F 6C 6C C0 00");
    }

    #[test]
    fn unicast_move_frame_bytes() {
        // Portal 8: {"m": [94848, 0]} — positive fixint target per msgpack11
        let body = Value::Map(vec![(
            key("m"),
            Value::Array(vec![Value::from(94_848), Value::from(0)]),
        )]);
        let msgpack = encode_envelope(8, &body);
        assert_eq!(hex(&msgpack), "93 08 00 81 A1 6D 92 CE 00 01 72 80 00");
    }

    #[test]
    fn fix8_header_matches_msgpack_c() {
        // RS485::makeHeader(-1) style header + fixstr "FW" body
        let bytes = encode_envelope_fix8(-1, &[0xA2, b'F', b'W']);
        assert_eq!(hex(&bytes), "93 D0 FF D0 00 A2 46 57");
    }

    #[test]
    fn roundtrip_both_encodings() {
        let body = Value::Map(vec![(key("m"), Value::Array(vec![Value::from(-94_848), Value::from(123)]))]);
        for bytes in [
            encode_envelope(5, &body),
            encode_envelope_fix8_value(5, &body),
        ] {
            let env = decode_envelope(&bytes).unwrap();
            assert_eq!(env.target, 5);
            assert_eq!(env.source, 0);
            assert_eq!(env.body, body);
            assert_eq!(env.trailer, Trailer::Absent, "a 3-element frame has no trailer");
        }
    }

    /// A `bl status` request to portal 3, byte for byte.
    ///
    /// Pinned rather than round-tripped because this is the frame a bootloader that this code has
    /// never met has to parse. Everything before `CD` is what the CRC covers.
    ///
    /// The trailing `D5 EF` was produced by compiling `crc16Update` **out of the firmware**
    /// (`PortalFW/lib/msgpack-arduino/src/msgpack/COBSRWStream.cpp`) and folding it over the 19
    /// bytes up to and including `CC 07`, not by running the Rust function below. A pin computed
    /// with the implementation it is pinning would agree with itself no matter which of the two
    /// ends was wrong.
    #[test]
    fn bootloader_status_request_bytes() {
        let body = Value::Map(vec![(
            key("bl"),
            Value::Map(vec![(key("q"), Value::from("status"))]),
        )]);
        let msgpack = encode_envelope_trailer(3, &body, 7);
        assert_eq!(
            hex(&msgpack),
            "95 03 00 81 A2 62 6C 81 A1 71 A6 73 74 61 74 75 73 CC 07 CD D5 EF"
        );

        // The CRC field is the last three bytes, and covers everything before itself.
        let covered = msgpack.len() - 3;
        assert_eq!(
            crate::crc::crc16_ccitt_false(&msgpack[..covered]),
            u16::from_be_bytes([msgpack[covered + 1], msgpack[covered + 2]])
        );
        assert_eq!(check_trailer(&msgpack), Trailer::Ok { seq: 7 });
    }

    /// The trailer survives COBS framing and decoding, which is the only form it is ever seen in.
    #[test]
    fn a_trailered_frame_survives_framing() {
        let body = Value::Map(vec![(key("bl"), Value::Map(vec![(key("q"), Value::from("run"))]))]);
        let msgpack = encode_envelope_trailer(-1, &body, 0);
        // Also firmware-computed, same method as `bootloader_status_request_bytes`.
        assert_eq!(&msgpack[msgpack.len() - 3..], &[0xCD, 0xB9, 0x88]);
        let framed = encode_frame(&msgpack);
        assert_eq!(*framed.last().unwrap(), 0, "delimited");
        assert!(!framed[..framed.len() - 1].contains(&0), "no embedded zero");

        let decoded = cobs_decode(&framed[..framed.len() - 1]).unwrap();
        assert_eq!(decoded, msgpack);
        let env = decode_envelope(&decoded).unwrap();
        assert_eq!(env.target, -1);
        assert_eq!(env.trailer, Trailer::Ok { seq: 0 });
    }

    /// Corruption anywhere in the covered region is caught, and the seq is not reported for a
    /// frame that failed -- a caller that matched on seq alone would otherwise accept it.
    #[test]
    fn every_single_byte_corruption_is_rejected() {
        let body = Value::Map(vec![(key("bl"), Value::Map(vec![(key("q"), Value::from("map"))]))]);
        let clean = encode_envelope_trailer(9, &body, 200);
        assert_eq!(check_trailer(&clean), Trailer::Ok { seq: 200 });

        for index in 0..clean.len() {
            // Skip the array header: changing it changes how many elements there are, which is a
            // different failure (a malformed frame, not a corrupted one).
            if index == 0 {
                continue;
            }
            let mut corrupted = clean.clone();
            corrupted[index] ^= 0x01;
            match check_trailer(&corrupted) {
                Trailer::Bad { .. } => {}
                Trailer::Absent => {
                    // Only acceptable when the flip destroyed a forced-width marker, since then
                    // there is genuinely no trailer to check any more.
                    let tail = corrupted.len() - TRAILER_BYTES;
                    assert!(
                        index == tail || index == tail + 2,
                        "byte {index} silently lost its trailer without being a width marker"
                    );
                }
                Trailer::Ok { .. } => panic!("corruption at byte {index} was not detected"),
            }
            assert!(
                !check_trailer(&corrupted).acceptable() || index >= clean.len() - TRAILER_BYTES,
                "a corrupted frame was reported as acceptable"
            );
        }
    }

    /// Four elements is not a trailer, and must not be mistaken for one. The firmware writes
    /// either three or five; anything else is a frame this code does not understand.
    #[test]
    fn a_four_element_frame_has_no_trailer() {
        let mut out = vec![0x94u8]; // fixarray(4)
        out.push(0x03);
        out.push(0x00);
        out.push(0xC0); // nil body
        out.push(0xCC);
        out.push(0x07);
        assert_eq!(check_trailer(&out), Trailer::Absent);
    }

    /// A body ending in bytes that look like a trailer, inside a 3-element frame, is not one.
    #[test]
    fn a_body_that_resembles_a_trailer_is_not_one() {
        let body = Value::Binary(vec![0xCC, 0x07, 0xCD, 0x12, 0x34]);
        let msgpack = encode_envelope(3, &body);
        assert_eq!(check_trailer(&msgpack), Trailer::Absent);
        assert_eq!(decode_envelope(&msgpack).unwrap().body, body);
    }

    /// Replies come back with the firmware's forced-int8 header; the trailer works the same way.
    #[test]
    fn a_reply_carries_its_own_trailer() {
        let body = Value::Map(vec![(key("bl"), Value::Map(vec![(key("ok"), Value::from(true))]))]);
        let msgpack = encode_reply_trailer(4, &body, 11);
        assert_eq!(&msgpack[..5], &[0x95, 0xD0, 0x00, 0xD0, 0x04]);
        let env = decode_envelope(&msgpack).unwrap();
        assert_eq!((env.target, env.source), (0, 4));
        assert_eq!(env.trailer, Trailer::Ok { seq: 11 });
    }

    /// The fix8 data-frame form, which is what a v6 firmware upload actually sends.
    #[test]
    fn a_data_frame_can_carry_a_trailer() {
        let body = [0x81, 0x00, 0xC4, 0x04, 0x00, 0x00, 0xAA, 0x55];
        let msgpack = encode_envelope_fix8_trailer(-1, &body, 3);
        assert_eq!(&msgpack[..5], &[0x95, 0xD0, 0xFF, 0xD0, 0x00]);
        assert_eq!(&msgpack[5..13], &body);
        assert_eq!(check_trailer(&msgpack), Trailer::Ok { seq: 3 });
    }
}
