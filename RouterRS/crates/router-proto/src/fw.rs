//! Firmware update protocol (`FWUpdate.cpp` / `MassFWUpdate`), and its v6 successor's data frames.
//!
//! The sequence is: alternate the two announce words until every running application has rebooted
//! and every bootloader is held resident, settle on the short word alone, erase, upload the binary
//! in fixed-size frames, then run. The ordering is not arbitrary and the reason it looks the way
//! it does is documented where it is built, in `router_link::fw_update` -- the short summary is
//! that the application and the bootloader listen for *different words*, so a phase carrying only
//! one of them leaves half the fleet in the wrong state.
//!
//! All of it is broadcast, with the msgpack-c style forced-int8 header.
//!
//! A v6 bootloader accepts these same frames, in any order, and additionally verifies a
//! `[seq, crc16]` trailer when one is present -- see [`fw_frame_envelope_trailer`].

use crate::envelope::{encode_envelope_fix8, encode_envelope_fix8_trailer};
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
    ///
    /// This word and [`FwMagic::Announce`] are **interleaved**, not sent in sequence: a phase
    /// carrying only the long word puts a board into a loop, because the application reboots into
    /// a bootloader that then hears nothing it can parse, times out after 3 s, and jumps straight
    /// back into the application. A v4/v5 bootloader rejects this word outright (it parses an
    /// announce with a 3-byte buffer, so 7 bytes is a format error); a v6 bootloader accepts any
    /// word beginning "FW" as a keepalive.
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
    encode_envelope_fix8(-1, &fw_frame_body(frame_offset, data))
}

/// The same frame with a `[seq, crc16]` trailer.
///
/// The payload's own XOR-16 checksum stays: it is what a v4/v5 bootloader checks, and dropping it
/// would make these frames unintelligible to the fielded fleet. The trailer is additional, covers
/// the *whole* frame rather than just the payload (a bit-flip in the offset key is undetectable to
/// XOR-16, and lands a chunk in the wrong place), and is what a v6 bootloader gates on.
pub fn fw_frame_envelope_trailer(frame_offset: u32, data: &[u8], seq: u8) -> Vec<u8> {
    encode_envelope_fix8_trailer(-1, &fw_frame_body(frame_offset, data), seq)
}

/// `{frame_offset: bin(checksum_le ++ data)}`.
fn fw_frame_body(frame_offset: u32, data: &[u8]) -> Vec<u8> {
    let checksum = checksum_xor16(data);

    let mut body = Vec::with_capacity(data.len() + 16);
    body.push(0x81); // fixmap(1)
    dump_uint(u64::from(frame_offset), &mut body);
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
    body
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

    /// The last 128-byte chunk of a completely full **v6** bank, which is 8 kB larger than the
    /// legacy one. Same uint32 path, one bank further out.
    #[test]
    fn a_v6_chunk_at_the_top_of_the_new_bank_still_encodes_a_uint32_offset() {
        use crate::layout;
        const CHUNK: u32 = 128;
        let last = layout::app_bank_bytes(layout::APP_BASE) as u32 - CHUNK;
        assert_eq!(last, 0x0001_A780);

        let data = [0xA5u8; 128];
        let env = fw_frame_envelope_trailer(last, &data, 5);
        assert_eq!(&env[..5], &[0x95, 0xD0, 0xFF, 0xD0, 0x00], "5-element envelope");
        assert_eq!(env[5], 0x81, "fixmap(1)");
        assert_eq!(env[6], 0xCE, "uint32 key");
        assert_eq!(&env[7..11], &last.to_be_bytes());
        assert_eq!(&env[7..11], &[0x00, 0x01, 0xA7, 0x80]);
        assert_eq!(env[11], 0xC4, "bin8");
        assert_eq!(env[12], 130, "2 checksum bytes + 128 data");
        assert_eq!(&env[15..15 + 128], &data[..]);

        // 64 identical LE words XOR to zero.
        assert_eq!(&env[13..15], &[0x00, 0x00]);
        assert_eq!(
            crate::envelope::check_trailer(&env),
            crate::envelope::Trailer::Ok { seq: 5 }
        );
    }

    /// The trailered and untrailered forms must carry an identical payload, or a v6 bootloader
    /// and a v4 bootloader would disagree about what was uploaded.
    #[test]
    fn adding_a_trailer_changes_nothing_but_the_envelope() {
        let data: Vec<u8> = (0u8..128).collect();
        let plain = fw_frame_envelope(1_024, &data);
        let trailered = fw_frame_envelope_trailer(1_024, &data, 9);
        assert_eq!(plain[0], 0x93, "3 elements");
        assert_eq!(trailered[0], 0x95, "5 elements");
        assert_eq!(&plain[1..], &trailered[1..plain.len()], "same header and body");
        assert_eq!(trailered.len(), plain.len() + 5);
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
