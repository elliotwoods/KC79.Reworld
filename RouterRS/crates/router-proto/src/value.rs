//! MessagePack serializer that byte-matches the two encoders used by the C++
//! Router app.
//!
//! `dump()` replicates `msgpack11::MsgPack::dump()` (`Router/src/msgpack11/
//! msgpack11.cpp`): integers are written in their minimal representation,
//! using unsigned families for non-negative values; `f32` is always `0xCA`.
//!
//! `write_fix_int8()` replicates `msgpack_pack_fix_int8` from msgpack-c,
//! which always emits `0xD0` — used by `RS485::makeHeader` and `FWUpdate`.

use rmpv::Value;

/// Serialize a value with msgpack11-compatible (minimal) encoding.
pub fn dump(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Nil => out.push(0xC0),
        Value::Boolean(b) => out.push(if *b { 0xC3 } else { 0xC2 }),
        Value::Integer(i) => {
            if let Some(u) = i.as_u64() {
                dump_uint(u, out);
            } else {
                dump_int(i.as_i64().expect("integer is i64 or u64"), out);
            }
        }
        Value::F32(f) => {
            out.push(0xCA);
            out.extend_from_slice(&f.to_be_bytes());
        }
        Value::F64(f) => {
            out.push(0xCB);
            out.extend_from_slice(&f.to_be_bytes());
        }
        Value::String(s) => {
            let bytes = s.as_bytes();
            dump_str_header(bytes.len(), out);
            out.extend_from_slice(bytes);
        }
        Value::Binary(b) => {
            dump_bin_header(b.len(), out);
            out.extend_from_slice(b);
        }
        Value::Array(items) => {
            dump_array_header(items.len(), out);
            for item in items {
                dump(item, out);
            }
        }
        Value::Map(entries) => {
            dump_map_header(entries.len(), out);
            for (k, v) in entries {
                dump(k, out);
                dump(v, out);
            }
        }
        Value::Ext(tag, data) => {
            // Not used on this wire; encode as standard ext for completeness.
            match data.len() {
                1 => out.push(0xD4),
                2 => out.push(0xD5),
                4 => out.push(0xD6),
                8 => out.push(0xD7),
                16 => out.push(0xD8),
                n if n < 256 => {
                    out.push(0xC7);
                    out.push(n as u8);
                }
                n if n < 65536 => {
                    out.push(0xC8);
                    out.extend_from_slice(&(n as u16).to_be_bytes());
                }
                n => {
                    out.push(0xC9);
                    out.extend_from_slice(&(n as u32).to_be_bytes());
                }
            }
            out.push(*tag as u8);
            out.extend_from_slice(data);
        }
    }
}

pub fn dump_to_vec(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    dump(value, &mut out);
    out
}

/// Non-negative integers: positive fixint / uint8 / uint16 / uint32 / uint64,
/// exactly as msgpack11's uint dump chain.
pub fn dump_uint(v: u64, out: &mut Vec<u8>) {
    if v < 128 {
        out.push(v as u8);
    } else if v < 256 {
        out.push(0xCC);
        out.push(v as u8);
    } else if v < 65_536 {
        out.push(0xCD);
        out.extend_from_slice(&(v as u16).to_be_bytes());
    } else if v < (1 << 32) {
        out.push(0xCE);
        out.extend_from_slice(&(v as u32).to_be_bytes());
    } else {
        out.push(0xCF);
        out.extend_from_slice(&v.to_be_bytes());
    }
}

/// Signed integers, msgpack11 dump chain: non-negative values route through
/// the unsigned families; negatives pick the smallest signed representation.
pub fn dump_int(v: i64, out: &mut Vec<u8>) {
    if v >= 0 {
        dump_uint(v as u64, out);
    } else if v >= -32 {
        out.push(v as i8 as u8); // negative fixint
    } else if v >= -128 {
        out.push(0xD0);
        out.push(v as i8 as u8);
    } else if v >= -32_768 {
        out.push(0xD1);
        out.extend_from_slice(&(v as i16).to_be_bytes());
    } else if v >= -(1i64 << 31) {
        out.push(0xD2);
        out.extend_from_slice(&(v as i32).to_be_bytes());
    } else {
        out.push(0xD3);
        out.extend_from_slice(&v.to_be_bytes());
    }
}

/// `msgpack_pack_fix_int8`: always `0xD0` + byte, regardless of value.
pub fn write_fix_int8(v: i8, out: &mut Vec<u8>) {
    out.push(0xD0);
    out.push(v as u8);
}

fn dump_str_header(len: usize, out: &mut Vec<u8>) {
    if len < 32 {
        out.push(0xA0 | len as u8);
    } else if len < 256 {
        out.push(0xD9);
        out.push(len as u8);
    } else if len < 65_536 {
        out.push(0xDA);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(0xDB);
        out.extend_from_slice(&(len as u32).to_be_bytes());
    }
}

fn dump_bin_header(len: usize, out: &mut Vec<u8>) {
    if len < 256 {
        out.push(0xC4);
        out.push(len as u8);
    } else if len < 65_536 {
        out.push(0xC5);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(0xC6);
        out.extend_from_slice(&(len as u32).to_be_bytes());
    }
}

fn dump_array_header(len: usize, out: &mut Vec<u8>) {
    if len < 16 {
        out.push(0x90 | len as u8);
    } else if len < 65_536 {
        out.push(0xDC);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(0xDD);
        out.extend_from_slice(&(len as u32).to_be_bytes());
    }
}

fn dump_map_header(len: usize, out: &mut Vec<u8>) {
    if len < 16 {
        out.push(0x80 | len as u8);
    } else if len < 65_536 {
        out.push(0xDE);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(0xDF);
        out.extend_from_slice(&(len as u32).to_be_bytes());
    }
}

/// Convenience constructors for building message bodies.
pub fn map(entries: Vec<(Value, Value)>) -> Value {
    Value::Map(entries)
}

pub fn key(name: &str) -> Value {
    Value::String(name.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn int_encodings_match_msgpack11() {
        let cases: &[(i64, &str)] = &[
            (0, "00"),
            (1, "01"),
            (127, "7F"),
            (128, "CC 80"),
            (255, "CC FF"),
            (256, "CD 01 00"),
            (65_535, "CD FF FF"),
            (65_536, "CE 00 01 00 00"),
            (94_848, "CE 00 01 72 80"), // MICROSTEPS/2, the "see through" target
            (-1, "FF"),
            (-32, "E0"),
            (-33, "D0 DF"),
            (-128, "D0 80"),
            (-129, "D1 FF 7F"),
            (-32_768, "D1 80 00"),
            (-32_769, "D2 FF FF 7F FF"),
            (-94_848, "D2 FF FE 8D 80"),
        ];
        for (v, expected) in cases {
            let mut out = Vec::new();
            dump_int(*v, &mut out);
            assert_eq!(hex(&out), *expected, "value {v}");
        }
    }

    #[test]
    fn f32_is_always_ca() {
        let mut out = Vec::new();
        dump(&Value::F32(0.25), &mut out);
        assert_eq!(out[0], 0xCA);
        assert_eq!(out.len(), 5);
    }

    #[test]
    fn fix_int8_is_forced_d0() {
        let mut out = Vec::new();
        write_fix_int8(-1, &mut out);
        write_fix_int8(0, &mut out);
        write_fix_int8(8, &mut out);
        assert_eq!(hex(&out), "D0 FF D0 00 D0 08");
    }
}
