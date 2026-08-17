//! COBS (Consistent Overhead Byte Stuffing) codec, ported from the vendored
//! `Router/src/cobs-c` implementation so behavior matches the C++ app exactly.
//! Frames on the wire are COBS-encoded msgpack followed by a `0x00` delimiter.

use crate::error::ProtoError;

/// COBS-encode `src` (no trailing delimiter).
pub fn cobs_encode(src: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len() + src.len() / 254 + 2);
    let mut code_idx = out.len();
    out.push(0); // placeholder for the first code byte
    let mut code: u8 = 1;

    for &byte in src {
        if byte == 0 {
            out[code_idx] = code;
            code_idx = out.len();
            out.push(0);
            code = 1;
        } else {
            out.push(byte);
            code += 1;
            if code == 0xFF {
                out[code_idx] = code;
                code_idx = out.len();
                out.push(0);
                code = 1;
            }
        }
    }
    out[code_idx] = code;
    out
}

/// COBS-encode and append the `0x00` frame delimiter — the exact bytes the
/// C++ `RS485::serialThreadSend` writes to the device.
pub fn encode_frame(msgpack: &[u8]) -> Vec<u8> {
    let mut out = cobs_encode(msgpack);
    out.push(0);
    out
}

/// Decode a COBS block (without delimiter).
pub fn cobs_decode(src: &[u8]) -> Result<Vec<u8>, ProtoError> {
    if src.is_empty() {
        return Err(ProtoError::CobsEmpty);
    }
    let mut out = Vec::with_capacity(src.len());
    let mut i = 0usize;
    while i < src.len() {
        let code = src[i];
        if code == 0 {
            return Err(ProtoError::CobsZeroInFrame { offset: i });
        }
        let block_len = (code - 1) as usize;
        if i + 1 + block_len > src.len() {
            return Err(ProtoError::CobsTruncated);
        }
        for j in 0..block_len {
            let byte = src[i + 1 + j];
            if byte == 0 {
                return Err(ProtoError::CobsZeroInFrame { offset: i + 1 + j });
            }
            out.push(byte);
        }
        i += 1 + block_len;
        // A non-0xFF code byte implies a zero byte, unless it terminates the frame.
        if code != 0xFF && i < src.len() {
            out.push(0);
        }
    }
    Ok(out)
}

/// Accumulates raw serial bytes and yields decoded msgpack payloads at each
/// `0x00` delimiter, mirroring `RS485::serialThreadReceive`.
#[derive(Default)]
pub struct FrameAccumulator {
    buf: Vec<u8>,
}

impl FrameAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed incoming bytes; returns one entry per completed frame:
    /// the COBS-decoded msgpack payload, or the decode error.
    /// Empty frames (consecutive delimiters) are skipped silently, matching
    /// the C++ behavior of ignoring zero-length accumulations.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Result<Vec<u8>, ProtoError>> {
        let mut results = Vec::new();
        for &byte in bytes {
            if byte == 0 {
                if !self.buf.is_empty() {
                    results.push(cobs_decode(&self.buf));
                    self.buf.clear();
                }
            } else {
                self.buf.push(byte);
            }
        }
        results
    }

    /// Bytes currently buffered awaiting a delimiter.
    pub fn pending(&self) -> usize {
        self.buf.len()
    }

    pub fn clear(&mut self) {
        self.buf.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_simple() {
        let cases: Vec<Vec<u8>> = vec![
            vec![],
            vec![0x00],
            vec![0x00, 0x00],
            vec![0x11, 0x22, 0x00, 0x33],
            vec![0x11, 0x00],
            vec![0x00, 0x11],
            (1..=255u8).collect(),          // no zeros, crosses the 254 block boundary
            (0..=255u8).collect(),          // contains a zero
            vec![1u8; 254],                 // exactly one full block
            vec![1u8; 255],
            vec![1u8; 508],
        ];
        for case in cases {
            let encoded = cobs_encode(&case);
            assert!(!encoded.contains(&0), "encoded must not contain zero: {case:?}");
            let decoded = cobs_decode(&encoded).unwrap();
            assert_eq!(decoded, case);
        }
    }

    #[test]
    fn known_vectors() {
        // Classic COBS test vectors
        assert_eq!(cobs_encode(&[0x00]), vec![0x01, 0x01]);
        assert_eq!(cobs_encode(&[0x00, 0x00]), vec![0x01, 0x01, 0x01]);
        assert_eq!(cobs_encode(&[0x11, 0x22, 0x00, 0x33]), vec![0x03, 0x11, 0x22, 0x02, 0x33]);
        assert_eq!(cobs_encode(&[0x11, 0x22, 0x33, 0x44]), vec![0x05, 0x11, 0x22, 0x33, 0x44]);
        assert_eq!(cobs_encode(&[0x11, 0x00, 0x00, 0x00]), vec![0x02, 0x11, 0x01, 0x01, 0x01]);
    }

    #[test]
    fn accumulator_splits_frames() {
        let frame1 = encode_frame(&[0x93, 0x01, 0x02]);
        let frame2 = encode_frame(&[0xC0]);
        let mut all = frame1.clone();
        all.extend_from_slice(&frame2);

        let mut acc = FrameAccumulator::new();
        // Feed in awkward chunk sizes
        let mut results = Vec::new();
        for chunk in all.chunks(3) {
            results.extend(acc.push(chunk));
        }
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].as_ref().unwrap(), &vec![0x93, 0x01, 0x02]);
        assert_eq!(results[1].as_ref().unwrap(), &vec![0xC0]);
        assert_eq!(acc.pending(), 0);
    }

    #[test]
    fn accumulator_reports_bad_frame_and_recovers() {
        let mut acc = FrameAccumulator::new();
        // A truncated COBS frame: code byte claims more data than present
        let results = acc.push(&[0x05, 0x11, 0x00]);
        assert_eq!(results.len(), 1);
        assert!(results[0].is_err());
        // Next valid frame still decodes
        let results = acc.push(&encode_frame(&[0x42]));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].as_ref().unwrap(), &vec![0x42]);
    }
}
