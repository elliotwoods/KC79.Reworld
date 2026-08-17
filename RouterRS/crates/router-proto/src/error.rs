use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProtoError {
    #[error("COBS decode error: zero byte inside frame at offset {offset}")]
    CobsZeroInFrame { offset: usize },
    #[error("COBS decode error: truncated frame (code byte ran past end)")]
    CobsTruncated,
    #[error("COBS decode error: empty frame")]
    CobsEmpty,
    #[error("msgpack decode error: {0}")]
    Msgpack(String),
    #[error("envelope is not an array of at least 3 elements")]
    BadEnvelope,
    #[error("envelope target/source is not an integer")]
    BadAddress,
}
