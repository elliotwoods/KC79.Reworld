//! Wire protocol for the KC79 "Reworld" portal installation.
//!
//! Byte-compatible with the C++ Router app (`Router/src`):
//! - Frames are MessagePack inside COBS, delimited by `0x00`.
//! - Envelope is a 3-element msgpack array `[target: i8, source: i8, body]`.
//!   Target `-1` = broadcast, `0` = host, `1..=127` = portal IDs.
//! - The C++ app has two serialization paths which differ in header bytes:
//!   msgpack11 (`Portal::sendToPortal`, `Column::broadcast`) emits minimal
//!   int encodings (positive/negative fixints for small addresses), whereas
//!   the msgpack-c path (`RS485::makeHeader`, `FWUpdate`) forces `0xD0` int8.
//!   Both are provided; the firmware accepts either.

pub mod cobs;
pub mod commands;
pub mod constants;
pub mod envelope;
pub mod error;
pub mod fw;
pub mod replies;
pub mod value;

pub use cobs::{cobs_decode, cobs_encode, encode_frame, FrameAccumulator};
pub use envelope::{decode_envelope, encode_envelope, encode_envelope_fix8, Envelope, BROADCAST, HOST};
pub use error::ProtoError;
pub use rmpv::Value;
