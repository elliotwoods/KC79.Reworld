//! The two checksums this protocol uses, and nothing else.
//!
//! They are different algorithms for different jobs and the pair is easy to confuse, so both live
//! here with the firmware source of each named:
//!
//! - [`crc16_ccitt_false`] protects a **frame in flight**. It is the trailer on a 5-element
//!   envelope, computed over every decoded byte before itself, and it must agree with
//!   `COBSRWStream`'s running CRC in `PortalFW/lib/msgpack-arduino/src/msgpack/COBSRWStream.cpp`
//!   -- the firmware folds it in a byte at a time as it reads, so a host that computed a
//!   different variant would simply see every frame rejected.
//! - [`crc32c`] protects **something durable**: a persistent flash record, the RAM handoff block,
//!   or a whole firmware image being verified after upload. It must agree with
//!   `PortalBootloader/include/portal_crc32c.h`, which `PortalFW/src/PersistentStorage.cpp` and
//!   the bootloader both include.
//!
//! Both are bitwise. Neither is on a path where throughput matters: the frame CRC covers tens of
//! bytes, and the image CRC runs once at the end of an upload that took tens of seconds.

/// CRC-16/CCITT-FALSE: polynomial 0x1021, init 0xFFFF, no reflection, xorout 0x0000.
///
/// The frame trailer. `crc16_ccitt_false(b"123456789") == 0x29B1`.
pub fn crc16_ccitt_false(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for byte in data {
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

/// CRC-32C (Castagnoli), reflected: polynomial 0x82F63B78, init and xorout 0xFFFFFFFF.
///
/// Every durable structure on a Portal. `crc32c(b"123456789") == 0xE306_9283`.
pub fn crc32c(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0x82F6_3B78 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The published check values. These are what make the two implementations on either side of
    /// the wire the *same* algorithm rather than two plausible ones.
    #[test]
    fn standard_vectors() {
        assert_eq!(crc16_ccitt_false(b"123456789"), 0x29B1);
        assert_eq!(crc16_ccitt_false(b""), 0xFFFF);
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
        assert_eq!(crc32c(b""), 0);
    }

    /// The firmware's CRC-32C is a `static inline` in a header both firmware images include, so
    /// the C source is available to read here. Checking the constants textually catches the one
    /// mistake a check vector would also catch but much later: someone "fixing" the polynomial to
    /// the more familiar 0xEDB88320 of zlib CRC-32.
    #[test]
    fn the_firmware_crc32c_uses_the_castagnoli_polynomial() {
        const HEADER: &str = include_str!("../../../../PortalBootloader/include/portal_crc32c.h");
        assert!(
            HEADER.contains("0x82F63B78"),
            "the firmware header is not CRC-32C"
        );
        assert!(
            !HEADER.contains("0xEDB88320"),
            "the firmware header mentions the zlib polynomial"
        );
    }

    /// Both are position-dependent: a transposition has to change the result, or the check is
    /// decorative.
    #[test]
    fn order_matters_to_both() {
        assert_ne!(crc16_ccitt_false(b"ab"), crc16_ccitt_false(b"ba"));
        assert_ne!(crc32c(b"ab"), crc32c(b"ba"));
    }

    /// A single bit flipped anywhere in a frame-sized buffer changes the CRC. This is the property
    /// the trailer is actually relied upon for, so it is worth asserting rather than assuming.
    #[test]
    fn every_single_bit_flip_is_detected() {
        let message = b"[3, 0, {\"bl\": {\"q\": \"status\"}}]";
        let clean16 = crc16_ccitt_false(message);
        let clean32 = crc32c(message);
        for index in 0..message.len() {
            for bit in 0..8 {
                let mut corrupted = message.to_vec();
                corrupted[index] ^= 1 << bit;
                assert_ne!(crc16_ccitt_false(&corrupted), clean16);
                assert_ne!(crc32c(&corrupted), clean32);
            }
        }
    }
}
