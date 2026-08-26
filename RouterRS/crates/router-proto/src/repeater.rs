//! The RS485 repeater control plane.
//!
//! V3 puts six ESP32-C3 repeaters between one shared outer bus and six isolated
//! nine-Portal branches. Until now they were invisible on the wire: no address, no
//! way to ask them anything, and no way to update them except a USB cable.
//!
//! # Addressing
//!
//! ```text
//! host     -> repeater : [0, 0,    {"rq": {"a": <addr>, "q": "<verb>", "v": <payload>}}]
//! repeater -> host     : [0, addr, {"rr": {"a": <addr>, "q": "<verb>", "ok": <bool>, "v": <payload>}}]
//! ```
//!
//! The envelope target is `0` — the host's own address — rather than the repeater
//! address, which looks odd until you check what a repeater running v2.2.0 does
//! with each option. `BridgeCore::shouldForward` drops every side-1 frame with
//! target `0` unconditionally, before it even consults its routing mode, whereas an
//! unrecognised address falls through to a fail-open `return true`. Putting the
//! repeater address in the *body* therefore means an un-updated repeater silently
//! ignores control traffic, rather than relaying a 300 kB firmware image onto nine
//! Portals. It also means Portals and the frozen STM32 bootloader never see these
//! frames at all.
//!
//! `a` is [`REPEATER_ALL`] or a unicast address from [`repeater_address`], or a
//! six-byte MAC, which reaches a unit whose index is unset or wrong.

use crate::envelope::{encode_envelope, HOST};
use crate::value::{key, map};
use crate::{ProtoError, Value};

/// Addresses every repeater at once. Only verbs that solicit no reply may use it;
/// six answers arriving together on a half-duplex multidrop bus is a collision.
pub const REPEATER_ALL: i8 = -2;

/// Repeaters in a V3 installation.
pub const REPEATER_COUNT: u8 = 6;

/// Bumped when the control-plane wire format changes. Read it from `status` before
/// using the snapshot or OTA verbs, and degrade per repeater rather than fleet-wide.
pub const CONTROL_PROTO_VERSION: u16 = 1;

/// Repeater `1..=6` maps to `-3..=-8`. Chosen so it can never collide with a Portal
/// ID, which is always positive, nor with `BROADCAST` (`-1`) or `HOST` (`0`).
pub fn repeater_address(index: u8) -> Option<i8> {
    if (1..=REPEATER_COUNT).contains(&index) {
        Some(-(2 + index as i8))
    } else {
        None
    }
}

/// The inverse of [`repeater_address`].
pub fn repeater_index(address: i8) -> Option<u8> {
    let index = -address - 2;
    if (1..=REPEATER_COUNT as i8).contains(&index) {
        Some(index as u8)
    } else {
        None
    }
}

/// Which repeater owns a given Portal ID, given the standard nine-per-branch layout.
pub fn repeater_for_portal(portal: u8) -> Option<u8> {
    if (1..=REPEATER_COUNT * 9).contains(&portal) {
        Some((portal - 1) / 9 + 1)
    } else {
        None
    }
}

/// The nine Portal IDs a repeater serves.
pub fn portal_range(index: u8) -> Option<(u8, u8)> {
    if (1..=REPEATER_COUNT).contains(&index) {
        let start = (index - 1) * 9 + 1;
        Some((start, start + 8))
    } else {
        None
    }
}

/// How a request names its recipient.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepeaterTarget {
    /// Every repeater. Legal only for verbs that answer nothing.
    All,
    /// One repeater by its provisioned index, `1..=6`.
    Index(u8),
    /// One repeater by MAC. Works even when its index is unset or wrong, which is
    /// the case that matters: a repeater whose branch is dead never learns a range.
    Mac([u8; 6]),
}

impl RepeaterTarget {
    fn to_value(&self) -> Value {
        match self {
            RepeaterTarget::All => Value::from(REPEATER_ALL),
            RepeaterTarget::Index(index) => Value::from(repeater_address(*index).unwrap_or(0)),
            RepeaterTarget::Mac(mac) => Value::Binary(mac.to_vec()),
        }
    }

    /// The address to match a reply against. `None` for a broadcast, which is
    /// never answered, and for a MAC, whose reply source depends on whether the
    /// unit has an index yet.
    pub fn reply_source(&self) -> Option<i8> {
        match self {
            RepeaterTarget::Index(index) => repeater_address(*index),
            _ => None,
        }
    }
}

/// The verbs a repeater understands. The names are the literal strings on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepeaterVerb {
    Status,
    Relearn,
    ResetCounters,
    Reboot,
    SetIndex,
    SnapshotStart,
    SnapshotRead,
    OtaBegin,
    OtaData,
    OtaMap,
    OtaEnd,
    OtaBoot,
    OtaConfirm,
    OtaAbort,
}

impl RepeaterVerb {
    pub fn as_str(self) -> &'static str {
        match self {
            RepeaterVerb::Status => "status",
            RepeaterVerb::Relearn => "relearn",
            RepeaterVerb::ResetCounters => "reset-counters",
            RepeaterVerb::Reboot => "reboot",
            RepeaterVerb::SetIndex => "set-index",
            RepeaterVerb::SnapshotStart => "snap-start",
            RepeaterVerb::SnapshotRead => "snap-read",
            RepeaterVerb::OtaBegin => "ota-begin",
            RepeaterVerb::OtaData => "ota-data",
            RepeaterVerb::OtaMap => "ota-map",
            RepeaterVerb::OtaEnd => "ota-end",
            RepeaterVerb::OtaBoot => "ota-boot",
            RepeaterVerb::OtaConfirm => "ota-confirm",
            RepeaterVerb::OtaAbort => "ota-abort",
        }
    }

    pub fn from_str(name: &str) -> Option<Self> {
        Some(match name {
            "status" => RepeaterVerb::Status,
            "relearn" => RepeaterVerb::Relearn,
            "reset-counters" => RepeaterVerb::ResetCounters,
            "reboot" => RepeaterVerb::Reboot,
            "set-index" => RepeaterVerb::SetIndex,
            "snap-start" => RepeaterVerb::SnapshotStart,
            "snap-read" => RepeaterVerb::SnapshotRead,
            "ota-begin" => RepeaterVerb::OtaBegin,
            "ota-data" => RepeaterVerb::OtaData,
            "ota-map" => RepeaterVerb::OtaMap,
            "ota-end" => RepeaterVerb::OtaEnd,
            "ota-boot" => RepeaterVerb::OtaBoot,
            "ota-confirm" => RepeaterVerb::OtaConfirm,
            "ota-abort" => RepeaterVerb::OtaAbort,
            _ => return None,
        })
    }

    /// Whether the repeater answers this verb. The firmware refuses to act on a
    /// reply-bearing verb sent to [`REPEATER_ALL`], so the host must not send one.
    pub fn expects_reply(self) -> bool {
        !matches!(
            self,
            RepeaterVerb::SnapshotStart
                | RepeaterVerb::OtaData
                | RepeaterVerb::OtaBoot
                | RepeaterVerb::OtaAbort
        )
    }
}

/// `[0, 0, {"rq": {...}}]`, ready for COBS framing.
pub fn request(target: &RepeaterTarget, verb: RepeaterVerb, payload: Option<Value>) -> Vec<u8> {
    let mut entries = vec![
        (key("a"), target.to_value()),
        (key("q"), Value::from(verb.as_str())),
    ];
    if let Some(payload) = payload {
        entries.push((key("v"), payload));
    }
    let body = map(vec![(key("rq"), map(entries))]);
    encode_envelope(HOST, &body)
}

/// A decoded `rr` reply.
#[derive(Debug, Clone, PartialEq)]
pub struct RepeaterReply {
    pub address: i8,
    pub verb: Option<RepeaterVerb>,
    pub ok: bool,
    pub payload: Option<Value>,
}

impl RepeaterReply {
    /// The repeater's index, or `None` if it is not yet provisioned (in which case
    /// it answers as [`REPEATER_ALL`] and identifies itself by MAC in the payload).
    pub fn index(&self) -> Option<u8> {
        repeater_index(self.address)
    }
}

/// Recognises a repeater reply in a decoded envelope body.
///
/// Returns `Ok(None)` for anything that simply is not one — ordinary Portal traffic
/// shares the `target == 0` envelope, so this is the common case, not an error.
pub fn parse_reply(body: &Value) -> Result<Option<RepeaterReply>, ProtoError> {
    let Value::Map(entries) = body else {
        return Ok(None);
    };
    let Some((_, inner)) = entries
        .iter()
        .find(|(k, _)| k.as_str() == Some("rr"))
    else {
        return Ok(None);
    };
    let Value::Map(fields) = inner else {
        return Err(ProtoError::Msgpack("rr is not a map".into()));
    };

    let mut reply = RepeaterReply {
        address: 0,
        verb: None,
        ok: false,
        payload: None,
    };
    for (k, v) in fields {
        match k.as_str() {
            Some("a") => {
                reply.address = v
                    .as_i64()
                    .and_then(|value| i8::try_from(value).ok())
                    .ok_or(ProtoError::BadAddress)?;
            }
            Some("q") => reply.verb = v.as_str().and_then(RepeaterVerb::from_str),
            Some("ok") => reply.ok = v.as_bool().unwrap_or(false),
            Some("v") => reply.payload = Some(v.clone()),
            _ => {}
        }
    }
    Ok(Some(reply))
}

/// CRC-16/CCITT-FALSE: poly `0x1021`, init `0xFFFF`, no reflection, xorout `0x0000`.
///
/// The definition pinned in `protocol-hardening.md`, the one the repeater firmware uses to check
/// each OTA chunk, and the one the Portal frame trailer uses. It moved to [`crate::crc`] when the
/// bootloader control plane became a second caller -- a checksum that two unrelated protocols
/// depend on byte-for-byte does not belong inside one of them. Re-exported here so the repeater
/// OTA code keeps its single import.
pub use crate::crc::crc16_ccitt_false;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cobs::encode_frame;
    use crate::envelope::decode_envelope;

    #[test]
    fn addresses_round_trip_and_never_collide_with_portals() {
        for index in 1..=REPEATER_COUNT {
            let address = repeater_address(index).unwrap();
            assert_eq!(address, -(2 + index as i8));
            assert_eq!(repeater_index(address), Some(index));
            // Never a Portal ID, never broadcast, never the host.
            assert!(address < -1);
        }
        assert_eq!(repeater_address(0), None);
        assert_eq!(repeater_address(7), None);
        assert_eq!(repeater_index(REPEATER_ALL), None);
        assert_eq!(repeater_index(crate::BROADCAST), None);
        assert_eq!(repeater_index(HOST), None);
        assert_eq!(repeater_index(5), None);
    }

    #[test]
    fn portal_to_repeater_mapping_covers_the_v3_topology() {
        assert_eq!(repeater_for_portal(1), Some(1));
        assert_eq!(repeater_for_portal(9), Some(1));
        assert_eq!(repeater_for_portal(10), Some(2));
        assert_eq!(repeater_for_portal(54), Some(6));
        assert_eq!(repeater_for_portal(55), None);
        assert_eq!(repeater_for_portal(0), None);

        assert_eq!(portal_range(1), Some((1, 9)));
        assert_eq!(portal_range(2), Some((10, 18)));
        assert_eq!(portal_range(6), Some((46, 54)));
        assert_eq!(portal_range(7), None);
    }

    #[test]
    fn a_status_request_is_addressed_to_the_host_not_the_repeater() {
        let bytes = request(&RepeaterTarget::Index(3), RepeaterVerb::Status, None);
        let envelope = decode_envelope(&bytes).unwrap();
        // This is the whole compatibility argument: target 0 is the one class a
        // v2.2.0 repeater refuses to forward to its branch, in every routing mode.
        assert_eq!(envelope.target, HOST);
        assert_eq!(envelope.source, HOST);

        let Value::Map(body) = &envelope.body else {
            panic!("body is not a map");
        };
        let (_, rq) = body.iter().find(|(k, _)| k.as_str() == Some("rq")).unwrap();
        let Value::Map(fields) = rq else {
            panic!("rq is not a map")
        };
        let address = fields.iter().find(|(k, _)| k.as_str() == Some("a")).unwrap();
        assert_eq!(address.1.as_i64(), Some(-5));
        let verb = fields.iter().find(|(k, _)| k.as_str() == Some("q")).unwrap();
        assert_eq!(verb.1.as_str(), Some("status"));
    }

    #[test]
    fn broadcast_and_mac_targets_encode_as_expected() {
        let all = request(&RepeaterTarget::All, RepeaterVerb::SnapshotStart, None);
        let envelope = decode_envelope(&all).unwrap();
        let Value::Map(body) = &envelope.body else { panic!() };
        let (_, rq) = body.iter().find(|(k, _)| k.as_str() == Some("rq")).unwrap();
        let Value::Map(fields) = rq else { panic!() };
        assert_eq!(
            fields.iter().find(|(k, _)| k.as_str() == Some("a")).unwrap().1.as_i64(),
            Some(REPEATER_ALL as i64)
        );

        let mac = [0xF8, 0x5B, 0x1B, 0xED, 0x8D, 0xA4];
        let by_mac = request(&RepeaterTarget::Mac(mac), RepeaterVerb::Status, None);
        let envelope = decode_envelope(&by_mac).unwrap();
        let Value::Map(body) = &envelope.body else { panic!() };
        let (_, rq) = body.iter().find(|(k, _)| k.as_str() == Some("rq")).unwrap();
        let Value::Map(fields) = rq else { panic!() };
        let address = &fields.iter().find(|(k, _)| k.as_str() == Some("a")).unwrap().1;
        assert_eq!(address.as_slice(), Some(&mac[..]));
    }

    #[test]
    fn only_reply_less_verbs_may_be_broadcast() {
        assert!(!RepeaterVerb::SnapshotStart.expects_reply());
        assert!(!RepeaterVerb::OtaData.expects_reply());
        assert!(!RepeaterVerb::OtaBoot.expects_reply());
        assert!(!RepeaterVerb::OtaAbort.expects_reply());
        assert!(RepeaterVerb::Status.expects_reply());
        assert!(RepeaterVerb::OtaBegin.expects_reply());
        assert!(RepeaterVerb::SnapshotRead.expects_reply());
    }

    #[test]
    fn replies_are_parsed_and_ordinary_traffic_is_not_mistaken_for_one() {
        let body = map(vec![(
            key("rr"),
            map(vec![
                (key("a"), Value::from(-5i8)),
                (key("q"), Value::from("status")),
                (key("ok"), Value::from(true)),
                (key("v"), map(vec![(key("proto"), Value::from(1))])),
            ]),
        )]);
        let reply = parse_reply(&body).unwrap().unwrap();
        assert_eq!(reply.address, -5);
        assert_eq!(reply.index(), Some(3));
        assert_eq!(reply.verb, Some(RepeaterVerb::Status));
        assert!(reply.ok);
        assert!(reply.payload.is_some());

        // A Portal position reply shares the `target == 0` envelope and must not be
        // mistaken for a repeater reply.
        let positions = map(vec![(
            key("p"),
            Value::Array(vec![Value::from(1), Value::from(2), Value::from(3), Value::from(4)]),
        )]);
        assert_eq!(parse_reply(&positions).unwrap(), None);
        assert_eq!(parse_reply(&Value::Boolean(true)).unwrap(), None);
    }

    #[test]
    fn an_unprovisioned_repeater_answers_as_the_broadcast_address() {
        let body = map(vec![(
            key("rr"),
            map(vec![
                (key("a"), Value::from(REPEATER_ALL)),
                (key("q"), Value::from("status")),
                (key("ok"), Value::from(true)),
            ]),
        )]);
        let reply = parse_reply(&body).unwrap().unwrap();
        assert_eq!(reply.address, REPEATER_ALL);
        assert_eq!(reply.index(), None);
    }

    #[test]
    fn crc16_matches_the_pinned_definition() {
        assert_eq!(crc16_ccitt_false(b"123456789"), 0x29B1);
        assert_eq!(crc16_ccitt_false(b""), 0xFFFF);
    }

    #[test]
    fn a_framed_request_carries_no_embedded_zero() {
        // Delimited at both ends; the COBS body between them carries no zero of its own.
        let framed = encode_frame(&request(&RepeaterTarget::Index(1), RepeaterVerb::Status, None));
        assert_eq!(*framed.first().unwrap(), 0);
        assert_eq!(*framed.last().unwrap(), 0);
        assert!(!framed[1..framed.len() - 1].contains(&0));
    }
}

#[cfg(test)]
mod frame_size_tests {
    use crate::cobs::encode_frame;
    use crate::commands::{keyframe, KeyframeValue};
    use crate::envelope::encode_envelope;

    /// The repeater's `MAX_FRAME_BYTES` dropped from 8192 to 2048 to bound how long
    /// a store-and-forward write blocks its loop. A frame above that is discarded
    /// atomically, so the limit has to clear the largest keyframe anyone can
    /// legitimately configure -- not just the nine-entry batch V3 uses.
    #[test]
    fn the_largest_configurable_keyframe_fits_the_repeater_frame_limit() {
        const REPEATER_MAX_FRAME_BYTES: usize = 2048;
        for count in [8usize, 9, 54] {
            // Worst case: full-range positions and velocities, so every integer
            // takes the widest encoding.
            let values: Vec<KeyframeValue> = (0..count)
                .map(|_| KeyframeValue::PosVel(189_704, -189_704, 189_704, -189_704))
                .collect();
            let framed = encode_frame(&encode_envelope(crate::BROADCAST, &keyframe(1, &values)));
            assert!(
                framed.len() < REPEATER_MAX_FRAME_BYTES,
                "{count}-entry keyframe is {} bytes, limit {REPEATER_MAX_FRAME_BYTES}",
                framed.len()
            );
            println!("{count}-entry keyframe: {} framed bytes", framed.len());
        }
    }
}
