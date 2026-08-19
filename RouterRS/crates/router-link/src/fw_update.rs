//! Firmware update over RS485 (port of `FWUpdate.cpp` / `MassFWUpdate`).
//!
//! The C++ implementation sends packets synchronously with promise-waits;
//! here the entire sequence is enqueued into the outbox as non-collateable
//! packets whose `custom_wait_time_ms` reproduces the original pacing:
//!
//! 1. announce "FW" x50 @ 100 ms      (reboots units into the bootloader)
//! 2. erase "ER", then announce x50
//! 3. data frames (32 bytes each), `wait_between_frames` apart, repeated
//!    `frame_repetitions` times
//! 4. (manually) run "RU"

use router_proto::constants::FW_FRAME_SIZE;
use router_proto::fw::{fw_frame_envelope, magic_word_envelope, FwMagic};

use crate::rs485::{Packet, Payload, Rs485};

/// Application bytes below the final three persistent flash pages.
pub const APP_BANK_BYTES: usize = 0x0801_E800 - 0x0800_6000;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FwUpdateError {
    #[error(
        "firmware image is {bytes} bytes; the persistent-safe application limit is {limit} bytes"
    )]
    TooLarge { bytes: usize, limit: usize },
    #[error("firmware frame size must be non-zero")]
    ZeroFrameSize,
}

fn validate(firmware: &[u8], params: &FwUpdateParams) -> Result<(), FwUpdateError> {
    if firmware.len() > APP_BANK_BYTES {
        return Err(FwUpdateError::TooLarge {
            bytes: firmware.len(),
            limit: APP_BANK_BYTES,
        });
    }
    if params.frame_size == 0 {
        return Err(FwUpdateError::ZeroFrameSize);
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct FwUpdateParams {
    pub announce_period_ms: u32,
    pub truncate: bool,
    pub frame_size: usize,
    pub wait_between_frames_ms: u32,
    pub frame_repetitions: u32,
}

impl Default for FwUpdateParams {
    fn default() -> Self {
        Self {
            announce_period_ms: 100,
            truncate: true,
            frame_size: FW_FRAME_SIZE,
            wait_between_frames_ms: 5,
            frame_repetitions: 1,
        }
    }
}

impl FwUpdateParams {
    /// MassFWUpdate defaults: slower frames, more repetitions.
    pub fn mass() -> Self {
        Self {
            wait_between_frames_ms: 10,
            frame_repetitions: 6,
            ..Default::default()
        }
    }
}

fn magic_packet(magic: FwMagic, wait_ms: u32) -> Packet {
    Packet {
        payload: Payload::Rendered(magic_word_envelope(magic)),
        target: -1,
        address: String::new(),
        needs_ack: false,
        collateable: false,
        custom_wait_time_ms: Some(wait_ms),
        on_sent: None,
    }
}

/// Enqueue the full upload sequence. Returns the number of packets queued.
pub fn upload(
    rs485: &Rs485,
    firmware: &[u8],
    params: &FwUpdateParams,
) -> Result<usize, FwUpdateError> {
    validate(firmware, params)?;
    let mut data = firmware.to_vec();
    if params.truncate {
        while data.last() == Some(&0xFF) {
            data.pop();
        }
    }

    rs485.clear_outbox();
    let mut count = 0usize;

    // announce x50 with the long word (reboot every running application to its
    // bootloader) -- see FwMagic::AnnounceLong.
    for _ in 0..50 {
        rs485.transmit(magic_packet(
            FwMagic::AnnounceLong,
            params.announce_period_ms,
        ));
        count += 1;
    }
    // erase, then announce x50 again with the short word: by now every application has
    // had the 5s above to see the long word and reboot, so this keeps re-announcing with
    // the bootloader's own (frozen, short) word instead, which is what it actually parses.
    rs485.transmit(magic_packet(FwMagic::Erase, params.announce_period_ms));
    count += 1;
    for _ in 0..50 {
        rs485.transmit(magic_packet(FwMagic::Announce, params.announce_period_ms));
        count += 1;
    }

    // data frames
    let mut offset = 0usize;
    while offset < data.len() {
        let end = (offset + params.frame_size).min(data.len());
        let frame = &data[offset..end];
        for _ in 0..params.frame_repetitions.max(1) {
            rs485.transmit(Packet {
                payload: Payload::Rendered(fw_frame_envelope(offset as u32, frame)),
                target: -1,
                address: String::new(),
                needs_ack: false,
                collateable: false,
                custom_wait_time_ms: Some(params.wait_between_frames_ms),
                on_sent: None,
            });
            count += 1;
        }
        offset = end;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistent_pages_are_outside_every_accepted_upload() {
        let params = FwUpdateParams::default();
        assert!(validate(&vec![0xFF; APP_BANK_BYTES], &params).is_ok());
        assert!(matches!(
            validate(&vec![0; APP_BANK_BYTES + 1], &params),
            Err(FwUpdateError::TooLarge { .. })
        ));
    }
}

/// Broadcast the "erase" magic word once.
pub fn erase(rs485: &Rs485, params: &FwUpdateParams) {
    rs485.transmit(magic_packet(FwMagic::Erase, params.announce_period_ms));
}

/// Broadcast the "run application" magic word once.
pub fn run_application(rs485: &Rs485, params: &FwUpdateParams) {
    rs485.transmit(magic_packet(FwMagic::Run, params.announce_period_ms));
}
