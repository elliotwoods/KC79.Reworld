//! The v6 bootloader control plane: `{"bl": {"q": <verb>, ...}}`.
//!
//! # Why a bootloader needs a control plane at all
//!
//! The fielded v4/v5 bootloader cannot speak. It accepts broadcast frames, writes them to flash in
//! strictly increasing order, and never transmits a byte. One dropped frame therefore ends an
//! upload silently: the bootloader refuses everything after the gap, the host keeps sending, and
//! both sides finish believing they succeeded. The only recovery available is to send every frame
//! several times and hope.
//!
//! v6 answers. It reports which chunks it actually received as a bitmap, so a host sends the image
//! once and repairs exactly the gaps, then asks it to verify a CRC-32C over the programmed bank
//! before running it. That is the same shape as the repeater's OTA protocol in [`crate::repeater`]
//! -- deliberately, because it is the shape that works on a lossy multidrop bus.
//!
//! # Addressing, and why replies are rationed
//!
//! Requests are addressed like ordinary traffic: `[id, 0, ...]` to one board, `[-1, 0, ...]` to
//! every board. Replies come back as `[0, id, ...]`, which is what the V3 repeaters already
//! forward upstream unchanged, so this works through them with no repeater firmware change.
//!
//! The bus is half-duplex, so **only one board may ever answer one frame**. A unicast request is
//! answered by its target. A broadcast request is answered by nobody -- unless it carries a
//! [`BlSelector`], which names one board by provisioning serial or MCU UID. That is the escape
//! hatch for the case the whole design has to survive: a board whose RS485 id is unknown, because
//! it power-cycled without an application to tell the bootloader what its id was.
//!
//! Broadcast without a selector is still useful, for the verbs that need no answer: `begin` and
//! the data frames themselves go to every board at once, which is what makes updating 54 boards
//! take one image transmission rather than 54.

use rmpv::Value;

use crate::envelope::encode_envelope_trailer;
use crate::error::ProtoError;
use crate::layout;
use crate::value::{key, map};

/// The body key that marks a control-plane frame.
pub const KEY: &str = "bl";

/// The verbs a v6 bootloader understands. The names are the literal strings on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlVerb {
    /// Identify: version, id, serial, UID, bank geometry, current state, installed application.
    Status,
    /// Erase the whole application bank and start a session. Answered when the erase completes.
    Begin,
    /// Report the received-chunk bitmap.
    Map,
    /// CRC-32C the programmed bank and compare against what `begin` declared.
    Verify,
    /// Start the application.
    Run,
    /// Adopt an RS485 id. Requires a selector when broadcast.
    Adopt,
    /// Reset the MCU.
    Reset,
}

impl BlVerb {
    pub fn as_str(self) -> &'static str {
        match self {
            BlVerb::Status => "status",
            BlVerb::Begin => "begin",
            BlVerb::Map => "map",
            BlVerb::Verify => "verify",
            BlVerb::Run => "run",
            BlVerb::Adopt => "adopt",
            BlVerb::Reset => "reset",
        }
    }

    pub fn from_str(name: &str) -> Option<Self> {
        Some(match name {
            "status" => BlVerb::Status,
            "begin" => BlVerb::Begin,
            "map" => BlVerb::Map,
            "verify" => BlVerb::Verify,
            "run" => BlVerb::Run,
            "adopt" => BlVerb::Adopt,
            "reset" => BlVerb::Reset,
            _ => return None,
        })
    }

    /// Whether a board answers this verb when it is addressed by it.
    ///
    /// Every verb is answered when unicast -- unlike the repeater plane, where some verbs are
    /// fire-and-forget. What varies is whether a *broadcast* of it is safe, which is
    /// [`BlVerb::safe_to_broadcast`].
    pub fn expects_reply(self) -> bool {
        true
    }

    /// Whether this verb may be broadcast without a selector.
    ///
    /// `adopt` is excluded because a board that adopted an id from an unaddressed broadcast would
    /// collide with every other board on the bus. The rest are excluded from *unselected*
    /// broadcast only in the sense that no reply comes back; the action still happens on every
    /// board, which is exactly what a fleet update wants for `begin` and `run`.
    pub fn safe_to_broadcast(self) -> bool {
        !matches!(self, BlVerb::Adopt)
    }
}

/// How a broadcast request names the single board that should answer it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BlSelector {
    /// No selector: a unicast request, or a broadcast nobody answers.
    #[default]
    None,
    /// The provisioning serial from the board's identity page.
    Serial(u32),
    /// The MCU's 96-bit unique id, for a board with no valid identity record.
    Uid([u8; 12]),
}

impl BlSelector {
    fn push_into(self, entries: &mut Vec<(Value, Value)>) {
        match self {
            BlSelector::None => {}
            BlSelector::Serial(serial) => entries.push((key("s"), Value::from(serial))),
            BlSelector::Uid(uid) => entries.push((key("uid"), Value::Binary(uid.to_vec()))),
        }
    }

    pub fn is_none(self) -> bool {
        matches!(self, BlSelector::None)
    }
}

/// Error codes a bootloader reports in a reply's `err` field.
///
/// Numeric on the wire because the bootloader has no room for strings and no `printf`; the names
/// live here, on the side that has both.
pub mod err {
    pub const NONE: u8 = 0;
    pub const FORMAT: u8 = 1;
    pub const CRC16: u8 = 2;
    pub const BOUNDS: u8 = 3;
    pub const ALIGN: u8 = 4;
    pub const XOR: u8 = 5;
    pub const PROGRAM: u8 = 6;
    pub const ERASE: u8 = 7;
    pub const BUSY: u8 = 8;
    pub const NO_APP: u8 = 9;
    pub const DESCRIPTOR_MISSING: u8 = 10;
    pub const DESCRIPTOR_BASE: u8 = 11;
    pub const IMAGE_CRC: u8 = 12;
    pub const UNKNOWN_VERB: u8 = 13;
    pub const SELECTOR_REQUIRED: u8 = 14;
    pub const BAD_PARAM: u8 = 15;
}

/// A human-readable name for an error code, for logs and operator-facing messages.
pub fn error_name(code: u8) -> &'static str {
    match code {
        err::NONE => "ok",
        err::FORMAT => "malformed frame",
        err::CRC16 => "frame CRC mismatch",
        err::BOUNDS => "offset or length outside the application bank",
        err::ALIGN => "offset not a multiple of 8",
        err::XOR => "frame payload checksum mismatch",
        err::PROGRAM => "flash program failed or the target was not erased",
        err::ERASE => "flash erase failed",
        err::BUSY => "busy erasing",
        err::NO_APP => "no valid application installed",
        err::DESCRIPTOR_MISSING => "the installed image has no application descriptor",
        err::DESCRIPTOR_BASE => "the installed image was linked for a different base address",
        err::IMAGE_CRC => "the programmed image does not match the CRC declared by begin",
        err::UNKNOWN_VERB => "unknown verb",
        err::SELECTOR_REQUIRED => "this verb needs a serial or uid selector when broadcast",
        err::BAD_PARAM => "a parameter was out of range",
        _ => "unknown error code",
    }
}

/// The bootloader's state, as reported by `status.st`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlState {
    /// Counting down to starting the application.
    Idle,
    /// Erasing the application bank; not yet accepting data.
    Erasing,
    /// Session open, accepting data frames.
    Receiving,
    /// Resident indefinitely: there is nothing valid to run, or a session is open.
    Held,
    Unknown(u8),
}

impl BlState {
    pub fn from_code(code: u8) -> Self {
        match code {
            0 => BlState::Idle,
            1 => BlState::Erasing,
            2 => BlState::Receiving,
            3 => BlState::Held,
            other => BlState::Unknown(other),
        }
    }

    pub fn code(self) -> u8 {
        match self {
            BlState::Idle => 0,
            BlState::Erasing => 1,
            BlState::Receiving => 2,
            BlState::Held => 3,
            BlState::Unknown(other) => other,
        }
    }

    /// Whether the bootloader will accept data frames right now.
    pub fn accepting_data(self) -> bool {
        matches!(self, BlState::Receiving)
    }
}

/// Where a bootloader got the RS485 id it is answering on.
///
/// This matters to a host: an id that came from the application via the RAM handoff is the board's
/// real bus address, while an id read from the DIP switches is a fallback that several boards on a
/// V3 branch may share. Preferring serial selectors over ids when this says `Dip` is what keeps a
/// duplicate-id branch from being addressed wrongly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlIdSource {
    Handoff,
    Adopt,
    Dip,
    Unknown,
}

impl BlIdSource {
    pub fn from_str(name: &str) -> Self {
        match name {
            "handoff" => BlIdSource::Handoff,
            "adopt" => BlIdSource::Adopt,
            "dip" => BlIdSource::Dip,
            _ => BlIdSource::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            BlIdSource::Handoff => "handoff",
            BlIdSource::Adopt => "adopt",
            BlIdSource::Dip => "dip",
            BlIdSource::Unknown => "unknown",
        }
    }

    /// Whether this id can be trusted to be unique on the bus.
    pub fn is_authoritative(self) -> bool {
        matches!(self, BlIdSource::Handoff | BlIdSource::Adopt)
    }
}

/// The application a bootloader reports as installed, read from its descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlApp {
    pub base: u32,
    pub version: String,
}

/// A decoded `status` reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlStatus {
    /// Control-plane version. 6 for the bootloader this module speaks to.
    pub version: u8,
    pub id: i8,
    pub id_source: BlIdSource,
    /// Provisioning serial, absent when the identity page is blank or belongs to another MCU.
    pub serial: Option<u32>,
    pub uid: Option<[u8; 12]>,
    /// Where this bootloader expects an application to start.
    pub base: u32,
    /// Application bank size in bytes.
    pub cap: u32,
    /// Largest data-frame payload it will accept.
    pub chunk: u32,
    pub state: BlState,
    /// Pages erased so far, while erasing.
    pub erase_progress: u32,
    /// Highest byte offset written this session.
    pub high_water: u32,
    /// Bytes accepted this session.
    pub received: u32,
    /// Last error, if any.
    pub err: Option<u8>,
    /// The installed application, if one is present and has a descriptor.
    pub app: Option<BlApp>,
}

/// Every shape a `bl` reply can take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlReply {
    Status(Box<BlStatus>),
    Begin { ok: bool, err: Option<u8> },
    Map { chunk: u32, len: u32, bitmap: Vec<u8> },
    Verify { ok: bool, crc32: u32, len: u32 },
    Run { ok: bool, err: Option<u8>, base: u32 },
    Adopt { id: i8 },
    Reset { ok: bool },
    /// A verb this host does not know. Forward-compatible by design: a newer bootloader answering
    /// a verb added after this build should not look like a protocol error.
    Other { verb: String },
}

impl BlReply {
    pub fn verb(&self) -> Option<BlVerb> {
        Some(match self {
            BlReply::Status(_) => BlVerb::Status,
            BlReply::Begin { .. } => BlVerb::Begin,
            BlReply::Map { .. } => BlVerb::Map,
            BlReply::Verify { .. } => BlVerb::Verify,
            BlReply::Run { .. } => BlVerb::Run,
            BlReply::Adopt { .. } => BlVerb::Adopt,
            BlReply::Reset { .. } => BlVerb::Reset,
            BlReply::Other { .. } => return None,
        })
    }

    /// The error a board reported, if it reported one.
    pub fn err(&self) -> Option<u8> {
        match self {
            BlReply::Begin { err, .. } | BlReply::Run { err, .. } => *err,
            BlReply::Status(status) => status.err,
            _ => None,
        }
    }
}

/// Build a request frame, ready for COBS framing.
///
/// `seq` is echoed back in the reply's trailer, so a host can tell which request a reply answers
/// -- the correlation the ordinary command protocol still lacks (`protocol-hardening.md`,
/// Finding 3).
pub fn request(
    target: i8,
    verb: BlVerb,
    selector: BlSelector,
    args: Vec<(Value, Value)>,
    seq: u8,
) -> Vec<u8> {
    let mut entries = vec![(key("q"), Value::from(verb.as_str()))];
    selector.push_into(&mut entries);
    entries.extend(args);
    encode_envelope_trailer(target, &map(vec![(key(KEY), map(entries))]), seq)
}

pub fn status(target: i8, selector: BlSelector, seq: u8) -> Vec<u8> {
    request(target, BlVerb::Status, selector, vec![], seq)
}

/// Open a session: erase the whole application bank and declare what is coming.
///
/// `base` is optional and defaults, on the bootloader side, to its own application base. Passing
/// it explicitly is how a legacy-base image is delivered over the v6 path, which is the only way
/// to get map/verify coverage on the transition application.
pub fn begin(target: i8, len: u32, crc32: u32, chunk: u32, base: Option<u32>, seq: u8) -> Vec<u8> {
    let mut args = vec![
        (key("len"), Value::from(len)),
        (key("crc"), Value::from(crc32)),
        (key("chunk"), Value::from(chunk)),
    ];
    if let Some(base) = base {
        args.push((key("base"), Value::from(base)));
    }
    request(target, BlVerb::Begin, BlSelector::None, args, seq)
}

/// Ask which chunks arrived. `chunk` overrides the session's chunk size for the bitmap's
/// granularity; `None` uses whatever `begin` declared.
pub fn map_request(target: i8, chunk: Option<u32>, seq: u8) -> Vec<u8> {
    let args = chunk
        .map(|chunk| vec![(key("chunk"), Value::from(chunk))])
        .unwrap_or_default();
    request(target, BlVerb::Map, BlSelector::None, args, seq)
}

pub fn verify(target: i8, seq: u8) -> Vec<u8> {
    request(target, BlVerb::Verify, BlSelector::None, vec![], seq)
}

pub fn run(target: i8, seq: u8) -> Vec<u8> {
    request(target, BlVerb::Run, BlSelector::None, vec![], seq)
}

/// Tell a board which RS485 id to answer on.
///
/// The selector is not optional in practice: this verb exists for boards whose id is unknown, and
/// the bootloader refuses a broadcast `adopt` that does not name one.
pub fn adopt(target: i8, selector: BlSelector, id: i8, seq: u8) -> Vec<u8> {
    request(
        target,
        BlVerb::Adopt,
        selector,
        vec![(key("id"), Value::from(id))],
        seq,
    )
}

pub fn reset(target: i8, seq: u8) -> Vec<u8> {
    request(target, BlVerb::Reset, BlSelector::None, vec![], seq)
}

/// How many chunks an image of `len` bytes occupies at `chunk` bytes each.
pub fn chunk_count(len: usize, chunk: usize) -> usize {
    if chunk == 0 {
        return 0;
    }
    len.div_ceil(chunk)
}

/// Which chunk indices a bitmap says are still missing.
///
/// A bitmap shorter than the image counts everything past its end as missing rather than silently
/// assuming it arrived. Bit `i` is chunk `i`, LSB-first within each byte -- the same convention as
/// the repeater's OTA bitmap, so the two can share a host-side repair loop.
pub fn missing_chunks(bitmap: &[u8], chunk_count: usize) -> Vec<usize> {
    (0..chunk_count)
        .filter(|index| {
            let byte = index / 8;
            byte >= bitmap.len() || bitmap[byte] & (1 << (index % 8)) == 0
        })
        .collect()
}

fn field<'a>(fields: &'a [(Value, Value)], name: &str) -> Option<&'a Value> {
    fields
        .iter()
        .find(|(k, _)| k.as_str() == Some(name))
        .map(|(_, v)| v)
}

fn u32_field(fields: &[(Value, Value)], name: &str) -> Option<u32> {
    field(fields, name)?.as_u64().and_then(|v| u32::try_from(v).ok())
}

fn u8_field(fields: &[(Value, Value)], name: &str) -> Option<u8> {
    field(fields, name)?.as_u64().and_then(|v| u8::try_from(v).ok())
}

fn i8_field(fields: &[(Value, Value)], name: &str) -> Option<i8> {
    field(fields, name)?.as_i64().and_then(|v| i8::try_from(v).ok())
}

fn bool_field(fields: &[(Value, Value)], name: &str) -> Option<bool> {
    field(fields, name)?.as_bool()
}

/// Recognise a `bl` reply in a decoded envelope body.
///
/// Returns `Ok(None)` for anything that simply is not one. Ordinary Portal replies share the
/// `[0, id, ...]` envelope, so "not a bootloader reply" is the common case rather than an error --
/// the same contract [`crate::repeater::parse_reply`] has, and for the same reason.
pub fn parse_reply(body: &Value) -> Result<Option<BlReply>, ProtoError> {
    let Value::Map(entries) = body else {
        return Ok(None);
    };
    let Some((_, inner)) = entries.iter().find(|(k, _)| k.as_str() == Some(KEY)) else {
        return Ok(None);
    };
    let Value::Map(fields) = inner else {
        return Err(ProtoError::Msgpack("bl is not a map".into()));
    };

    let verb_name = field(fields, "q")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ProtoError::Msgpack("bl reply has no verb".into()))?;
    let Some(verb) = BlVerb::from_str(verb_name) else {
        return Ok(Some(BlReply::Other {
            verb: verb_name.to_string(),
        }));
    };

    let error = u8_field(fields, "err").filter(|code| *code != err::NONE);

    Ok(Some(match verb {
        BlVerb::Status => {
            let uid = match field(fields, "uid") {
                Some(Value::Binary(bytes)) if bytes.len() == 12 => {
                    Some(<[u8; 12]>::try_from(bytes.as_slice()).expect("length checked"))
                }
                _ => None,
            };
            let app = match field(fields, "app") {
                Some(Value::Map(app_fields)) => u32_field(app_fields, "base").map(|base| BlApp {
                    base,
                    version: field(app_fields, "ver")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                }),
                _ => None,
            };
            BlReply::Status(Box::new(BlStatus {
                version: u8_field(fields, "v").unwrap_or(0),
                id: i8_field(fields, "id").unwrap_or(0),
                id_source: field(fields, "src")
                    .and_then(|v| v.as_str())
                    .map_or(BlIdSource::Unknown, BlIdSource::from_str),
                // Serial 0 is not a valid provisioning serial (`PersistentStorage::readIdentity`
                // rejects it), so a board with no identity can report it as an absent value
                // without needing nil.
                serial: u32_field(fields, "s").filter(|serial| *serial != 0),
                uid,
                base: u32_field(fields, "base").unwrap_or(0),
                cap: u32_field(fields, "cap").unwrap_or(0),
                chunk: u32_field(fields, "chunk").unwrap_or(0),
                state: BlState::from_code(u8_field(fields, "st").unwrap_or(0)),
                erase_progress: u32_field(fields, "prog").unwrap_or(0),
                high_water: u32_field(fields, "wp").unwrap_or(0),
                received: u32_field(fields, "n").unwrap_or(0),
                err: error,
                app,
            }))
        }
        BlVerb::Begin => BlReply::Begin {
            ok: bool_field(fields, "ok").unwrap_or(false),
            err: error,
        },
        BlVerb::Map => BlReply::Map {
            chunk: u32_field(fields, "chunk").unwrap_or(0),
            len: u32_field(fields, "len").unwrap_or(0),
            bitmap: match field(fields, "map") {
                Some(Value::Binary(bytes)) => bytes.clone(),
                _ => Vec::new(),
            },
        },
        BlVerb::Verify => BlReply::Verify {
            ok: bool_field(fields, "ok").unwrap_or(false),
            crc32: u32_field(fields, "crc").unwrap_or(0),
            len: u32_field(fields, "len").unwrap_or(0),
        },
        BlVerb::Run => BlReply::Run {
            ok: bool_field(fields, "ok").unwrap_or(false),
            err: error,
            base: u32_field(fields, "base").unwrap_or(0),
        },
        BlVerb::Adopt => BlReply::Adopt {
            id: i8_field(fields, "id").unwrap_or(0),
        },
        BlVerb::Reset => BlReply::Reset {
            ok: bool_field(fields, "ok").unwrap_or(false),
        },
    }))
}

/// Whether a `status` reply came from a bootloader this host knows how to drive.
pub fn speaks_v6(status: &BlStatus) -> bool {
    status.version >= layout::BL_PROTO_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cobs::encode_frame;
    use crate::envelope::{decode_envelope, encode_reply_trailer, Trailer};

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
    }

    /// Decode a request the way a bootloader would, so the tests below assert against the wire
    /// rather than against the builders' own arguments.
    fn inner_fields(frame: &[u8]) -> Vec<(Value, Value)> {
        let env = decode_envelope(frame).unwrap();
        assert!(env.trailer.acceptable(), "request carried a bad trailer");
        let Value::Map(entries) = env.body else { panic!("body is not a map") };
        assert_eq!(entries.len(), 1, "exactly one body key");
        assert_eq!(entries[0].0.as_str(), Some(KEY));
        let Value::Map(fields) = &entries[0].1 else { panic!("bl is not a map") };
        fields.clone()
    }

    fn verb_of(frame: &[u8]) -> String {
        field(&inner_fields(frame), "q").unwrap().as_str().unwrap().to_string()
    }

    #[test]
    fn verb_names_round_trip() {
        for verb in [
            BlVerb::Status,
            BlVerb::Begin,
            BlVerb::Map,
            BlVerb::Verify,
            BlVerb::Run,
            BlVerb::Adopt,
            BlVerb::Reset,
        ] {
            assert_eq!(BlVerb::from_str(verb.as_str()), Some(verb));
        }
        assert_eq!(BlVerb::from_str("ota-begin"), None, "not a repeater verb");
        assert_eq!(BlVerb::from_str(""), None);
    }

    /// The exact bytes of a `status` request, as the firmware's own CRC computes them.
    #[test]
    fn status_request_bytes() {
        assert_eq!(
            hex(&status(3, BlSelector::None, 7)),
            "95 03 00 81 A2 62 6C 81 A1 71 A6 73 74 61 74 75 73 CC 07 CD D5 EF"
        );
    }

    #[test]
    fn every_verb_encodes_its_arguments() {
        let opened = begin(2, 108_544, 0xDEAD_BEEF, 128, None, 1);
        let fields = inner_fields(&opened);
        assert_eq!(verb_of(&opened), "begin");
        assert_eq!(u32_field(&fields, "len"), Some(108_544));
        assert_eq!(u32_field(&fields, "crc"), Some(0xDEAD_BEEF));
        assert_eq!(u32_field(&fields, "chunk"), Some(128));
        assert_eq!(field(&fields, "base"), None, "base omitted when defaulted");

        let with_base = begin(2, 8, 0, 128, Some(layout::APP_BASE_LEGACY), 1);
        assert_eq!(
            u32_field(&inner_fields(&with_base), "base"),
            Some(layout::APP_BASE_LEGACY)
        );

        assert_eq!(verb_of(&map_request(2, None, 0)), "map");
        assert_eq!(field(&inner_fields(&map_request(2, None, 0)), "chunk"), None);
        assert_eq!(
            u32_field(&inner_fields(&map_request(2, Some(32), 0)), "chunk"),
            Some(32)
        );

        assert_eq!(verb_of(&verify(2, 0)), "verify");
        assert_eq!(verb_of(&run(2, 0)), "run");
        assert_eq!(verb_of(&reset(2, 0)), "reset");

        let adopted = adopt(-1, BlSelector::Serial(73_001), 5, 0);
        assert_eq!(verb_of(&adopted), "adopt");
        assert_eq!(i8_field(&inner_fields(&adopted), "id"), Some(5));
    }

    /// Selectors are what make a broadcast answerable by exactly one board, so their encoding is
    /// checked against the wire rather than assumed.
    #[test]
    fn selectors_encode_as_serial_or_binary_uid() {
        let none = inner_fields(&status(4, BlSelector::None, 0));
        assert_eq!(field(&none, "s"), None);
        assert_eq!(field(&none, "uid"), None);

        let serial = inner_fields(&status(-1, BlSelector::Serial(73_001), 0));
        assert_eq!(u32_field(&serial, "s"), Some(73_001));
        assert_eq!(field(&serial, "uid"), None);

        let uid_bytes: [u8; 12] = [
            0x44, 0x33, 0x22, 0x11, 0x88, 0x77, 0x66, 0x55, 0xCC, 0xBB, 0xAA, 0x99,
        ];
        let uid = inner_fields(&status(-1, BlSelector::Uid(uid_bytes), 0));
        assert_eq!(field(&uid, "s"), None);
        assert_eq!(field(&uid, "uid"), Some(&Value::Binary(uid_bytes.to_vec())));
    }

    /// Every request has to survive COBS framing, which means it must contain no zero byte before
    /// the delimiter. A `begin` carrying a CRC with a zero byte in it is the case that would
    /// break a naive framer, so it is the one tested.
    #[test]
    fn framed_requests_carry_no_embedded_zero() {
        for frame in [
            status(3, BlSelector::None, 0),
            begin(0, 0x0001_0000, 0x0000_00FF, 128, None, 0),
            map_request(1, None, 0),
            verify(1, 0),
            run(1, 0),
            adopt(-1, BlSelector::Uid([0; 12]), 1, 0),
            reset(1, 0),
        ] {
            // A frame is delimited at both ends now; the body between them is what must be
            // free of zeros, because that is what COBS guarantees and what a receiver relies on
            // to find the boundaries.
            let framed = encode_frame(&frame);
            assert_eq!(*framed.first().unwrap(), 0);
            assert_eq!(*framed.last().unwrap(), 0);
            assert!(
                !framed[1..framed.len() - 1].contains(&0),
                "embedded zero in {}",
                hex(&framed)
            );
        }
    }

    fn reply_body(fields: Vec<(Value, Value)>) -> Value {
        map(vec![(key(KEY), map(fields))])
    }

    #[test]
    fn a_status_reply_parses_every_field() {
        let body = reply_body(vec![
            (key("q"), Value::from("status")),
            (key("v"), Value::from(6)),
            (key("id"), Value::from(3)),
            (key("src"), Value::from("handoff")),
            (key("s"), Value::from(73_001)),
            (
                key("uid"),
                Value::Binary(vec![
                    0x44, 0x33, 0x22, 0x11, 0x88, 0x77, 0x66, 0x55, 0xCC, 0xBB, 0xAA, 0x99,
                ]),
            ),
            (key("base"), Value::from(layout::APP_BASE)),
            (key("cap"), Value::from(108_544)),
            (key("chunk"), Value::from(256)),
            (key("st"), Value::from(2)),
            (key("prog"), Value::from(53)),
            (key("wp"), Value::from(4_096)),
            (key("n"), Value::from(4_096)),
            (
                key("app"),
                map(vec![
                    (key("base"), Value::from(layout::APP_BASE)),
                    (key("ver"), Value::from("Portal v2026-08-25_19.19 ea08436+")),
                ]),
            ),
        ]);
        let Some(BlReply::Status(status)) = parse_reply(&body).unwrap() else {
            panic!("not a status reply");
        };
        assert_eq!(status.version, 6);
        assert!(speaks_v6(&status));
        assert_eq!(status.id, 3);
        assert_eq!(status.id_source, BlIdSource::Handoff);
        assert!(status.id_source.is_authoritative());
        assert_eq!(status.serial, Some(73_001));
        assert_eq!(status.uid.unwrap()[0], 0x44);
        assert_eq!(status.base, layout::APP_BASE);
        assert_eq!(status.cap, 108_544);
        assert_eq!(status.chunk, 256);
        assert_eq!(status.state, BlState::Receiving);
        assert!(status.state.accepting_data());
        assert_eq!(status.erase_progress, 53);
        assert_eq!(status.high_water, 4_096);
        assert_eq!(status.received, 4_096);
        assert_eq!(status.err, None);
        assert_eq!(status.app.unwrap().base, layout::APP_BASE);
    }

    /// A board with no identity and no application: every optional field absent. This is what a
    /// virgin board answers, and it must not parse as an error.
    #[test]
    fn a_minimal_status_reply_parses() {
        let body = reply_body(vec![
            (key("q"), Value::from("status")),
            (key("v"), Value::from(6)),
            (key("id"), Value::from(0)),
            (key("src"), Value::from("dip")),
            (key("s"), Value::from(0)),
            (key("base"), Value::from(layout::APP_BASE)),
            (key("cap"), Value::from(108_544)),
            (key("chunk"), Value::from(256)),
            (key("st"), Value::from(3)),
            (key("err"), Value::from(err::NO_APP)),
        ]);
        let Some(BlReply::Status(status)) = parse_reply(&body).unwrap() else {
            panic!("not a status reply");
        };
        assert_eq!(status.serial, None, "serial 0 means no identity");
        assert_eq!(status.uid, None);
        assert_eq!(status.app, None);
        assert_eq!(status.state, BlState::Held);
        assert_eq!(status.err, Some(err::NO_APP));
        assert_eq!(error_name(err::NO_APP), "no valid application installed");
        assert!(!status.id_source.is_authoritative(), "a DIP id is a fallback");
    }

    #[test]
    fn the_other_replies_parse() {
        let cases: Vec<(Vec<(Value, Value)>, BlReply)> = vec![
            (
                vec![(key("q"), Value::from("begin")), (key("ok"), Value::from(true))],
                BlReply::Begin { ok: true, err: None },
            ),
            (
                vec![
                    (key("q"), Value::from("begin")),
                    (key("ok"), Value::from(false)),
                    (key("err"), Value::from(err::BAD_PARAM)),
                ],
                BlReply::Begin {
                    ok: false,
                    err: Some(err::BAD_PARAM),
                },
            ),
            (
                vec![
                    (key("q"), Value::from("map")),
                    (key("chunk"), Value::from(128)),
                    (key("len"), Value::from(384)),
                    (key("map"), Value::Binary(vec![0b0000_0101])),
                ],
                BlReply::Map {
                    chunk: 128,
                    len: 384,
                    bitmap: vec![0b0000_0101],
                },
            ),
            (
                vec![
                    (key("q"), Value::from("verify")),
                    (key("ok"), Value::from(true)),
                    (key("crc"), Value::from(0xDEAD_BEEFu32)),
                    (key("len"), Value::from(108_544)),
                ],
                BlReply::Verify {
                    ok: true,
                    crc32: 0xDEAD_BEEF,
                    len: 108_544,
                },
            ),
            (
                vec![
                    (key("q"), Value::from("run")),
                    (key("ok"), Value::from(false)),
                    (key("err"), Value::from(err::DESCRIPTOR_BASE)),
                    (key("base"), Value::from(layout::APP_BASE)),
                ],
                BlReply::Run {
                    ok: false,
                    err: Some(err::DESCRIPTOR_BASE),
                    base: layout::APP_BASE,
                },
            ),
            (
                vec![(key("q"), Value::from("adopt")), (key("id"), Value::from(9))],
                BlReply::Adopt { id: 9 },
            ),
            (
                vec![(key("q"), Value::from("reset")), (key("ok"), Value::from(true))],
                BlReply::Reset { ok: true },
            ),
        ];
        for (fields, want) in cases {
            assert_eq!(parse_reply(&reply_body(fields)).unwrap(), Some(want));
        }
    }

    /// Ordinary Portal traffic shares the reply envelope, so anything that is not a `bl` map has
    /// to come back as "not one of mine" rather than as an error.
    #[test]
    fn ordinary_traffic_is_not_mistaken_for_a_bootloader_reply() {
        for body in [
            Value::Boolean(true),
            Value::Nil,
            map(vec![(key("p"), Value::Array(vec![Value::from(1)]))]),
            map(vec![(key("rr"), map(vec![(key("a"), Value::from(-3))]))]),
        ] {
            assert_eq!(parse_reply(&body).unwrap(), None);
        }
    }

    /// A verb added by a future bootloader must parse as `Other`, not as a decode failure -- the
    /// same forward-compatibility the envelope's extra elements give.
    #[test]
    fn an_unknown_verb_is_reported_rather_than_rejected() {
        let body = reply_body(vec![
            (key("q"), Value::from("teleport")),
            (key("ok"), Value::from(true)),
        ]);
        assert_eq!(
            parse_reply(&body).unwrap(),
            Some(BlReply::Other {
                verb: "teleport".into()
            })
        );
    }

    #[test]
    fn a_bl_body_that_is_not_a_map_is_an_error() {
        let body = map(vec![(key(KEY), Value::from(7))]);
        assert!(parse_reply(&body).is_err());
        let no_verb = reply_body(vec![(key("ok"), Value::from(true))]);
        assert!(parse_reply(&no_verb).is_err());
    }

    /// A whole reply, as it arrives: framed, trailered, from a board's own address.
    #[test]
    fn a_reply_round_trips_through_the_envelope() {
        let body = reply_body(vec![
            (key("q"), Value::from("verify")),
            (key("ok"), Value::from(true)),
            (key("crc"), Value::from(0x1234_5678u32)),
            (key("len"), Value::from(64)),
        ]);
        let frame = encode_reply_trailer(3, &body, 7);
        let env = decode_envelope(&frame).unwrap();
        assert_eq!((env.target, env.source), (0, 3));
        assert_eq!(env.trailer, Trailer::Ok { seq: 7 });
        assert_eq!(
            parse_reply(&env.body).unwrap(),
            Some(BlReply::Verify {
                ok: true,
                crc32: 0x1234_5678,
                len: 64
            })
        );
    }

    #[test]
    fn chunk_counting_and_repair_maths() {
        assert_eq!(chunk_count(0, 128), 0);
        assert_eq!(chunk_count(1, 128), 1);
        assert_eq!(chunk_count(128, 128), 1);
        assert_eq!(chunk_count(129, 128), 2);
        assert_eq!(chunk_count(108_544, 128), 848);
        assert_eq!(chunk_count(10, 0), 0, "a zero chunk size cannot divide");

        assert_eq!(missing_chunks(&[0b1011_1101], 8), vec![1, 6]);
        assert_eq!(missing_chunks(&[0xFF], 8), Vec::<usize>::new());
        assert_eq!(missing_chunks(&[0x00], 3), vec![0, 1, 2]);
        // Short bitmaps mean "missing", never "assume present".
        assert_eq!(missing_chunks(&[0xFF], 10), vec![8, 9]);
        assert_eq!(missing_chunks(&[], 2), vec![0, 1]);
    }

    /// A full-bank image's bitmap is 106 bytes, which comfortably fits one frame -- the property
    /// that lets `map` be a single request/reply rather than a paged one.
    #[test]
    fn a_full_bank_bitmap_fits_in_one_reply() {
        let chunks = chunk_count(layout::app_bank_bytes(layout::APP_BASE), layout::BL_CHUNK_MAX);
        assert_eq!(chunks, 424);
        let bitmap_bytes = chunks.div_ceil(8);
        assert_eq!(bitmap_bytes, 53);
        // Well inside the repeater's 2048-byte frame limit even with envelope overhead.
        assert!(bitmap_bytes + 64 < 2048);
    }

    #[test]
    fn broadcast_safety_matches_the_firmware_rule() {
        assert!(!BlVerb::Adopt.safe_to_broadcast());
        for verb in [
            BlVerb::Status,
            BlVerb::Begin,
            BlVerb::Map,
            BlVerb::Verify,
            BlVerb::Run,
            BlVerb::Reset,
        ] {
            assert!(verb.safe_to_broadcast());
        }
    }
}
