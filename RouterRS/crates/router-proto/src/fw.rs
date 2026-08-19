//! Firmware update protocol (`FWUpdate.cpp` / `MassFWUpdate`).
//!
//! Sequence: broadcast the "FW" magic word repeatedly (announce), "ER"
//! (erase), then upload the binary in 32-byte frames, then "RU" (run).
//! All packets are broadcast with the msgpack-c style forced-int8 header.

use crate::envelope::encode_envelope_fix8;
use crate::value::dump_uint;

/// Magic words broadcast as bare strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FwMagic {
    /// "FW": the bootloader's own announce word -- frozen (field-burned, only
    /// re-flashable via ST-Link), so this stays exactly 2 bytes forever. Resets its
    /// writePosition tracking. A device still running its application ignores this;
    /// it only reboots on `AnnounceLong`.
    Announce,
    /// "FW!KC79": reboots a running application into its bootloader. Longer and more
    /// improbable than the bare "FW" so a corrupted frame that happens to decode as a
    /// 2-byte match can't bounce a device mid-move (protocol-hardening.md Finding 4).
    /// Send this first in an update sequence to bump every running app; once they've
    /// had time to reboot, continue announcing with the short `Announce` word instead,
    /// which is what the (already-entered) bootloader actually parses.
    AnnounceLong,
    /// "ER": erase application flash
    Erase,
    /// "RU": run application
    Run,
}

impl FwMagic {
    pub fn word(self) -> &'static str {
        match self {
            FwMagic::Announce => "FW",
            FwMagic::AnnounceLong => "FW!KC79",
            FwMagic::Erase => "ER",
            FwMagic::Run => "RU",
        }
    }
}

/// XOR-of-u16-words checksum (`Utils::calcCheckSum`). The C++ implementation
/// walks 16-bit words (little-endian on x86); an odd trailing byte in C++
/// reads one byte past the buffer (UB) — here it is treated as a final word
/// with a zero high byte. FW frames are 32 bytes so this only differs for an
/// odd-sized final frame.
pub fn checksum_xor16(data: &[u8]) -> u16 {
    let mut value: u16 = 0;
    let mut chunks = data.chunks_exact(2);
    for chunk in &mut chunks {
        value ^= u16::from_le_bytes([chunk[0], chunk[1]]);
    }
    if let [last] = chunks.remainder() {
        value ^= u16::from_le_bytes([*last, 0]);
    }
    value
}

/// Envelope bytes (not COBS framed) for a magic word broadcast:
/// `[0x93, 0xD0, 0xFF, 0xD0, 0x00, fixstr(2), ...]`.
pub fn magic_word_envelope(magic: FwMagic) -> Vec<u8> {
    let word = magic.word().as_bytes();
    let mut body = Vec::with_capacity(3);
    body.push(0xA0 | word.len() as u8);
    body.extend_from_slice(word);
    encode_envelope_fix8(-1, &body)
}

/// Envelope bytes for one firmware data frame:
/// `[-1, 0, {frame_offset: bin(checksum_le ++ data)}]`.
/// The map key is the frame's byte offset (packed minimally, as msgpack-c
/// `msgpack_pack_uint32` does); the value is a bin of the 2-byte LE checksum
/// followed by the frame data.
pub fn fw_frame_envelope(frame_offset: u32, data: &[u8]) -> Vec<u8> {
    let checksum = checksum_xor16(data);

    let mut body = Vec::with_capacity(data.len() + 16);
    body.push(0x81); // fixmap(1)
    dump_uint(frame_offset as u64, &mut body);
    // bin header
    let len = data.len() + 2;
    if len < 256 {
        body.push(0xC4);
        body.push(len as u8);
    } else {
        body.push(0xC5);
        body.extend_from_slice(&(len as u16).to_be_bytes());
    }
    body.extend_from_slice(&checksum.to_le_bytes());
    body.extend_from_slice(data);

    encode_envelope_fix8(-1, &body)
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
    fn checksum_vectors() {
        assert_eq!(checksum_xor16(&[]), 0);
        assert_eq!(checksum_xor16(&[0x01, 0x02]), 0x0201);
        assert_eq!(checksum_xor16(&[0x01, 0x02, 0x01, 0x02]), 0);
        assert_eq!(checksum_xor16(&[0xFF; 32]), 0);
        // 32 incrementing bytes, computed by hand-XOR of LE words
        let data: Vec<u8> = (0u8..32).collect();
        let mut expected: u16 = 0;
        for pair in data.chunks(2) {
            expected ^= u16::from_le_bytes([pair[0], pair[1]]);
        }
        assert_eq!(checksum_xor16(&data), expected);
    }

    #[test]
    fn magic_word_bytes() {
        assert_eq!(
            hex(&magic_word_envelope(FwMagic::Announce)),
            "93 D0 FF D0 00 A2 46 57"
        );
        assert_eq!(
            hex(&magic_word_envelope(FwMagic::AnnounceLong)),
            "93 D0 FF D0 00 A7 46 57 21 4B 43 37 39"
        );
        assert_eq!(
            hex(&magic_word_envelope(FwMagic::Erase)),
            "93 D0 FF D0 00 A2 45 52"
        );
        assert_eq!(
            hex(&magic_word_envelope(FwMagic::Run)),
            "93 D0 FF D0 00 A2 52 55"
        );
    }

    #[test]
    fn fw_frame_layout() {
        let data = [0xAAu8; 32];
        let env = fw_frame_envelope(64, &data);
        // header: 93 D0 FF D0 00, then fixmap(1), key 64 (fixint 0x40),
        // bin8 header C4 22 (34 bytes), checksum LE (AAAA^... 16 words of AAAA = 0), data
        assert_eq!(&env[..5], &[0x93, 0xD0, 0xFF, 0xD0, 0x00]);
        assert_eq!(env[5], 0x81);
        assert_eq!(env[6], 0x40);
        assert_eq!(env[7], 0xC4);
        assert_eq!(env[8], 34);
        // 16 x 0xAAAA XORed = 0 (even count)
        assert_eq!(&env[9..11], &[0x00, 0x00]);
        assert_eq!(&env[11..], &data[..]);
    }

    #[test]
    fn fw_frame_offset_encoding_grows() {
        // offset 100_000 -> uint32 (0xCE)
        let env = fw_frame_envelope(100_000, &[0u8; 4]);
        assert_eq!(env[6], 0xCE);
    }

    /// The last 32-byte frame of a completely full application bank.
    ///
    /// The bank is 0x08006000..0x0801E800 = 100,352 bytes, so the final frame starts at
    /// 100,320 — well past the 16-bit boundary. This pins the whole >64 kB path: the key is
    /// a full uint32 with the exact big-endian bytes the bootloader's `readIntU32` reverses
    /// back out, and the frame that follows it is unremarkable. If anything ever narrows the
    /// offset to 16 bits, this is the test that fails.
    #[test]
    fn fw_frame_offset_spans_full_application_bank() {
        const APP_BANK_BYTES: u32 = 0x0001_E800 - 0x0000_6000; // 100,352
        const LAST_FRAME_OFFSET: u32 = APP_BANK_BYTES - 32; // 100,320

        let data = [0xA5u8; 32];
        let env = fw_frame_envelope(LAST_FRAME_OFFSET, &data);

        assert_eq!(&env[..5], &[0x93, 0xD0, 0xFF, 0xD0, 0x00]);
        assert_eq!(env[5], 0x81, "fixmap(1)");
        assert_eq!(env[6], 0xCE, "uint32 key, not uint16");
        assert_eq!(&env[7..11], &LAST_FRAME_OFFSET.to_be_bytes());
        assert_eq!(&env[7..11], &[0x00, 0x01, 0x87, 0xE0]);
        assert_eq!(env[11], 0xC4, "bin8");
        assert_eq!(env[12], 34, "2 checksum bytes + 32 data");
        assert_eq!(&env[15..], &data[..]);

        // 16 identical LE words XOR to zero, so the checksum is a stable literal here.
        assert_eq!(&env[13..15], &[0x00, 0x00]);
    }

    /// Every offset boundary the msgpack encoder crosses, so a regression narrows here rather
    /// than 60 kB into a bench upload.
    #[test]
    fn fw_frame_offset_encoding_at_every_width_boundary() {
        for (offset, want_marker) in [
            (0u32, 0x00u8),  // positive fixint
            (127, 0x7F),     // last fixint
            (128, 0xCC),     // uint8
            (255, 0xCC),     // last uint8
            (256, 0xCD),     // uint16
            (65_535, 0xCD),  // last uint16
            (65_536, 0xCE),  // uint32 - the boundary the folklore is about
            (100_320, 0xCE), // last frame of the persistent-safe bank
        ] {
            let env = fw_frame_envelope(offset, &[0u8; 4]);
            assert_eq!(
                env[6], want_marker,
                "offset {offset} encoded as {:#04X}, expected {want_marker:#04X}",
                env[6]
            );
        }
    }
}
