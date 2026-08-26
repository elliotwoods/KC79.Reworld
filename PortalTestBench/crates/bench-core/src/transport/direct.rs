//! Versioned binary protocol for the production firmware's USART1 Direct Mode.
//!
//! The human menu remains the power-on default. Records are MessagePack inside COBS frames.

use crate::dut::Axis;
use router_proto::Value;

pub const VERSION: u8 = 1;
pub const MAX_FRAME: usize = 64 * 1024;

pub mod kind {
    pub const HELLO: u8 = 1;
    pub const HEARTBEAT: u8 = 2;
    pub const EXIT: u8 = 3;
    pub const STATUS: u8 = 4;
    pub const OP: u8 = 5;
    pub const JOG: u8 = 6;
    pub const SURVEY_START: u8 = 7;
    pub const ABORT: u8 = 8;
    pub const ACK: u8 = 64;
    pub const ERROR: u8 = 65;
    pub const STATUS_EVENT: u8 = 66;
    pub const LOG_EVENT: u8 = 67;
    pub const SURVEY_BEGIN: u8 = 68;
    pub const SURVEY_SAMPLE: u8 = 69;
    pub const SURVEY_END: u8 = 70;
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionMode {
    #[default]
    Menu,
    Entering,
    Direct,
    Exiting,
    Fault,
}

impl SessionMode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Menu => "menu",
            Self::Entering => "entering",
            Self::Direct => "direct",
            Self::Exiting => "exiting",
            Self::Fault => "fault",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SurveyMode {
    #[default]
    Fast,
    Settled,
}

impl SurveyMode {
    pub fn wire(self) -> u8 {
        match self {
            Self::Fast => 0,
            Self::Settled => 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurveyConfig {
    pub axis: Axis,
    pub mode: SurveyMode,
    pub center: i32,
    pub center_is_home: bool,
    pub half_range: i32,
    pub step: i32,
    pub duty_min: u8,
    pub duty_max: u8,
}

impl SurveyConfig {
    pub fn sample_count(&self) -> Result<usize, String> {
        if self.half_range <= 0 || self.step <= 0 {
            return Err("range and step must be positive".into());
        }
        if self.duty_min > self.duty_max {
            return Err("duty range is reversed".into());
        }
        let count = ((self.half_range as usize) * 2 / self.step as usize) + 1;
        if count > 4096 {
            return Err("survey exceeds 4096 samples".into());
        }
        Ok(count)
    }
}

impl SurveyConfig {
    pub fn body(&self) -> Value {
        Value::Array(vec![
            Value::from(match self.axis {
                Axis::A => 0,
                Axis::B => 1,
            }),
            Value::from(self.mode.wire()),
            Value::from(self.center),
            Value::Boolean(self.center_is_home),
            Value::from(self.half_range),
            Value::from(self.step),
            Value::from(self.duty_min),
            Value::from(self.duty_max),
        ])
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SampleClass {
    Measured,
    CensoredBright,
    CensoredDark,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SurveySample {
    pub index: u32,
    pub position: i32,
    pub offset: i32,
    pub crossing: Option<u8>,
    pub class: SampleClass,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub seq: u8,
    pub kind: u8,
    pub body: Value,
}

pub fn encode(seq: u8, kind: u8, body: Value) -> Vec<u8> {
    let mut payload = vec![0x95];
    router_proto::value::dump(&Value::from(VERSION), &mut payload);
    router_proto::value::dump(&Value::from(seq), &mut payload);
    router_proto::value::dump(&Value::from(kind), &mut payload);
    router_proto::value::dump(&body, &mut payload);
    let crc = crc16(&payload);
    payload.push(0xcd);
    payload.extend_from_slice(&crc.to_be_bytes());
    router_proto::encode_frame(&payload)
}

pub fn decode(payload: &[u8]) -> Result<Frame, String> {
    if payload.len() < 4 || payload[payload.len() - 3] != 0xcd {
        return Err("direct frame has no forced-width CRC trailer".into());
    }
    let expected = u16::from_be_bytes([payload[payload.len() - 2], payload[payload.len() - 1]]);
    let actual = crc16(&payload[..payload.len() - 3]);
    if actual != expected {
        return Err("direct CRC mismatch".into());
    }
    let mut cursor = payload;
    let value = rmpv::decode::read_value(&mut cursor).map_err(|error| error.to_string())?;
    let Value::Array(items) = value else {
        return Err("direct frame is not an array".into());
    };
    if items.len() != 5 {
        return Err("direct frame must have five fields".into());
    }
    if items[0].as_u64() != Some(u64::from(VERSION)) {
        return Err("unsupported direct protocol version".into());
    }
    let seq = u8::try_from(items[1].as_u64().ok_or("direct sequence is invalid")?)
        .map_err(|_| "direct sequence is invalid")?;
    let kind = u8::try_from(items[2].as_u64().ok_or("direct kind is invalid")?)
        .map_err(|_| "direct kind is invalid")?;
    Ok(Frame {
        seq,
        kind,
        body: items[3].clone(),
    })
}

pub fn crc16(bytes: &[u8]) -> u16 {
    let mut crc = 0xffffu16;
    for byte in bytes {
        crc ^= u16::from(*byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frame_round_trips_through_cobs_and_crc() {
        let wire = encode(
            17,
            kind::JOG,
            Value::Array(vec![Value::from(0), Value::from(-14080)]),
        );
        // `encode_frame` writes a delimiter on *both* sides of the payload now -- the
        // leading one terminates the spurious byte an RS485 receiver samples at a
        // half-duplex turn-around, before the real frame starts. Strip both.
        let payload = router_proto::cobs_decode(&wire[1..wire.len() - 1]).unwrap();
        let frame = decode(&payload).unwrap();
        assert_eq!((frame.seq, frame.kind), (17, kind::JOG));
    }
    #[test]
    fn corruption_is_rejected() {
        let wire = encode(1, kind::HEARTBEAT, Value::Nil);
        // `encode_frame` writes a delimiter on *both* sides of the payload now -- the
        // leading one terminates the spurious byte an RS485 receiver samples at a
        // half-duplex turn-around, before the real frame starts. Strip both.
        let mut payload = router_proto::cobs_decode(&wire[1..wire.len() - 1]).unwrap();
        payload[2] ^= 1;
        assert!(decode(&payload).unwrap_err().contains("CRC mismatch"));
    }
    #[test]
    fn survey_configuration_is_bounded() {
        let ok = SurveyConfig {
            axis: Axis::A,
            mode: SurveyMode::Fast,
            center: 0,
            center_is_home: true,
            half_range: 500,
            step: 10,
            duty_min: 200,
            duty_max: 255,
        };
        assert_eq!(ok.sample_count(), Ok(101));
        assert!(
            SurveyConfig {
                step: 0,
                ..ok.clone()
            }
            .sample_count()
            .is_err()
        );
        assert!(
            SurveyConfig {
                half_range: 50_000,
                step: 1,
                ..ok
            }
            .sample_count()
            .is_err()
        );
    }
}
