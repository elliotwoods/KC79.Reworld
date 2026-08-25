//! The 3-element message envelope `[target, source, body]`.

use rmpv::Value;

use crate::error::ProtoError;
use crate::value::{dump, dump_int, dump_to_vec, write_fix_int8};

/// Broadcast address (all portals on the bus).
pub const BROADCAST: i8 = -1;
/// Host (router) address.
pub const HOST: i8 = 0;

#[derive(Debug, Clone, PartialEq)]
pub struct Envelope {
    pub target: i8,
    pub source: i8,
    pub body: Value,
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
    Ok(Envelope { target, source, body })
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
        }
    }
}
