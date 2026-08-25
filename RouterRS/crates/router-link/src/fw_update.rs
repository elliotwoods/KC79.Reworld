//! Firmware update over RS485 (port of `FWUpdate.cpp` / `MassFWUpdate`).
//!
//! The C++ implementation sends packets synchronously with promise-waits; here the whole
//! sequence is enqueued into the outbox as non-collateable packets whose
//! `custom_wait_time_ms` carries the pacing.
//!
//! # Why the announce phase looks the way it does
//!
//! Two devices listen to this traffic and they understand *different words*.
//!
//! A running application only acts on the long `"FW!KC79"`
//! (`PortalFW/src/Modules/RS485.cpp`), waits 500 ms, and resets. The bootloader cannot
//! parse that word at all -- `FWUpdateApp::processIncoming` reads an announce with
//! `allocatedSize = 3`, so a 7-byte string is rejected as a format error -- and only the
//! short `"FW"` resets its write position and extends its residency.
//!
//! The bootloader's residency is **3 s from reset**, extended to 10 s only by an
//! *accepted* frame (`PortalBootloader/.../main.cpp`). So a phase that sends nothing but
//! the long word puts every board into a loop: the application reboots into a bootloader
//! that then hears 3 s of words it cannot read, times out, and jumps straight back into
//! the application. Whether a given board happens to be in its bootloader when the short
//! word finally starts is a race on its own phase, and a board that loses that race is
//! never recalled -- the short word does not reboot applications -- so it sits out the
//! entire update while the host reports success.
//!
//! Interleaving the two words removes the race: an application always hears a word that
//! reboots it, and a bootloader always hears a word that holds it resident. Neither
//! device is harmed by the other's word (the application ignores a 2-byte string; the
//! bootloader logs one format error to its debug UART).
//!
//! The phases are therefore:
//!
//! 1. **bump**   -- `"FW!KC79"` and `"FW"` alternating, until every application has
//!    rebooted and every bootloader is held resident.
//! 2. **settle** -- `"FW"` only, long enough for a board that reset 500 ms after the last
//!    long announce to be resident before anything destructive happens.
//! 3. **erase**  -- `"ER"`, then `"FW"` to cover the blocking page erase and reset
//!    `writePosition`. Repeated once: erase is idempotent, and a board that arrived a
//!    little late would otherwise take data frames into unerased flash and die on its
//!    first `HAL_FLASH_Program`.
//! 4. **data**   -- 32-byte frames, `wait_between_frames_ms` apart, each repeated
//!    `frame_repetitions` times.
//! 5. **run**    -- `"RU"`, if `run_after` is set.
//!
//! # Why the sequence is split in two
//!
//! Phases 1-2 ([`announce_steps`]) only *recall* boards; phases 3-5 ([`upload_steps`]) are
//! what destroys the application bank. [`crate::fw_session`] has to interpose between the
//! two -- it asks every recalled board what it is before deciding whether this blind path
//! is the right one at all -- so the split is what lets it reuse this sequence verbatim
//! rather than re-deriving the announce timing that the tests below pin.

use router_proto::constants::FW_FRAME_SIZE;
use router_proto::fw::{fw_frame_envelope, magic_word_envelope, FwMagic};
use router_proto::layout;

use crate::rs485::{Packet, Payload, Rs485};

/// Flash is programmed a double-word at a time, so an upload is padded to a multiple of
/// this before it is framed. See [`prepare_image`].
pub const FLASH_WRITE_GRANULE: usize = layout::FLASH_GRANULE;

/// How long the two words alternate for. Covers an application's 500 ms reboot delay
/// several times over, so every board has been bumped at least once.
const BUMP_MS: u32 = 5_000;
/// Short-word-only tail of the announce, so a board that reset just after the last long
/// word is resident and window-extended before the erase.
const SETTLE_MS: u32 = 2_000;
/// Short announces after each erase. `flash_erase` blocks the bootloader for 1-2 s with
/// its receive ring overflowing, so this has to outlast the erase and then land at least
/// one parseable `"FW"` to put `writePosition` back to zero.
const ERASE_COVER_MS: u32 = 3_000;
/// Erase is idempotent; a second one costs the host nothing and covers a board whose
/// reboot took longer than expected.
const ERASE_PASSES: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FwUpdateError {
    #[error(
        "firmware image is {bytes} bytes; the persistent-safe application limit at \
         0x{base:08X} is {limit} bytes"
    )]
    TooLarge {
        bytes: usize,
        limit: usize,
        base: u32,
    },
    #[error("0x{base:08X} is not an application base address")]
    NotAnApplicationBase { base: u32 },
    #[error("firmware frame size must be non-zero")]
    ZeroFrameSize,
    #[error("firmware frame size must be even, so the host and the bootloader XOR the same 16-bit words")]
    OddFrameSize,
}

#[derive(Debug, Clone)]
pub struct FwUpdateParams {
    pub announce_period_ms: u32,
    pub truncate: bool,
    pub frame_size: usize,
    pub wait_between_frames_ms: u32,
    pub frame_repetitions: u32,
    /// Queue `"RU"` at the tail of the same outbox, so the application handoff cannot be
    /// forgotten. The GUI leaves this off and drives its own separate button.
    pub run_after: bool,
}

impl Default for FwUpdateParams {
    fn default() -> Self {
        Self {
            announce_period_ms: 100,
            truncate: true,
            frame_size: FW_FRAME_SIZE,
            wait_between_frames_ms: 5,
            frame_repetitions: 1,
            run_after: false,
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

    /// The bench profile: the slower frame gap, and enough repetition that one corrupted
    /// or dropped frame is survivable. A duplicate offset costs the bootloader nothing
    /// (`logPrint("R")`, skipped), so repetition is the only recovery available on a
    /// protocol with no acknowledgement.
    pub fn resilient() -> Self {
        Self {
            wait_between_frames_ms: 10,
            frame_repetitions: 2,
            ..Default::default()
        }
    }
}

/// One packet of the sequence, independent of the outbox.
///
/// This exists so the ordering and pacing can be asserted in a unit test: the sequence is
/// the part of this module that has to be right, and it is not observable once the
/// packets are inside a worker thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FwStep {
    Magic { magic: FwMagic, wait_ms: u32 },
    Data { offset: u32, len: usize, wait_ms: u32 },
}

impl FwStep {
    pub fn wait_ms(self) -> u32 {
        match self {
            FwStep::Magic { wait_ms, .. } | FwStep::Data { wait_ms, .. } => wait_ms,
        }
    }
}

/// Truncate trailing erased-state bytes, then pad back up to a whole double-word.
///
/// Both halves matter, and the padding is not cosmetic. `flash_write` programs whole
/// double-words: the fielded v4 bootloader advances a raw `uint64_t*` and reads however
/// many bytes past the caller's stack buffer the final chunk is short, then programs that
/// stack garbage into flash. A `0x08006000`-linked application whose length is not a
/// multiple of 8 -- which is most of them -- therefore lands with a non-deterministic
/// tail, and any byte-exact readback check fails on exactly those bytes even when the
/// transfer itself was perfect.
///
/// Padding with `0xFF` also guarantees an even-length final frame, which keeps
/// `checksum_xor16` and the firmware's `calcCheckSum` XORing the same 16-bit words.
pub fn prepare_image(firmware: &[u8], params: &FwUpdateParams) -> Vec<u8> {
    let mut data = firmware.to_vec();
    if params.truncate {
        while data.last() == Some(&0xFF) {
            data.pop();
        }
    }
    let remainder = data.len() % FLASH_WRITE_GRANULE;
    if remainder != 0 {
        data.resize(data.len() + (FLASH_WRITE_GRANULE - remainder), 0xFF);
    }
    data
}

/// Check an already-prepared image against the bank it is destined for.
///
/// The limit is a function of `base` rather than a constant, because the fleet has two
/// application banks that differ by 8 kB ([`layout`]): an image that fits a v6 board is
/// 8 kB too large for a v4/v5 one, and accepting it would program over the provisioning
/// serial and the settings journals.
///
/// Public because it is also what the addressed path in [`crate::fw_session`] checks with
/// -- the size and framing rules are properties of the flash map and of the frame format,
/// not of which protocol carries them.
pub fn validate(image: &[u8], base: u32, params: &FwUpdateParams) -> Result<(), FwUpdateError> {
    if !layout::is_app_base(base) {
        return Err(FwUpdateError::NotAnApplicationBase { base });
    }
    let limit = layout::app_bank_bytes(base);
    if image.len() > limit {
        return Err(FwUpdateError::TooLarge {
            bytes: image.len(),
            limit,
            base,
        });
    }
    if params.frame_size == 0 {
        return Err(FwUpdateError::ZeroFrameSize);
    }
    if !params.frame_size.is_multiple_of(2) {
        return Err(FwUpdateError::OddFrameSize);
    }
    Ok(())
}

/// The full packet sequence for an upload, in order.
pub fn plan(
    firmware: &[u8],
    base: u32,
    params: &FwUpdateParams,
) -> Result<Vec<FwStep>, FwUpdateError> {
    plan_for_image(&prepare_image(firmware, params), base, params)
}

/// `plan`, for an image that has already been through [`prepare_image`].
fn plan_for_image(
    image: &[u8],
    base: u32,
    params: &FwUpdateParams,
) -> Result<Vec<FwStep>, FwUpdateError> {
    let mut steps = announce_steps(params);
    steps.extend(upload_steps(image, base, params)?);
    Ok(steps)
}

/// Phases 1-2: the words that recall a fleet, and nothing that writes flash.
///
/// Safe to send to a bus whose composition is still unknown, which is what
/// [`crate::fw_session`] does before it has decided which path to take.
pub fn announce_steps(params: &FwUpdateParams) -> Vec<FwStep> {
    let period = params.announce_period_ms.max(1);
    let announce = |magic| FwStep::Magic { magic, wait_ms: period };
    let mut steps = Vec::new();

    // 1. bump: alternate, so applications reboot and bootloaders stay resident.
    for i in 0..(BUMP_MS / period) {
        steps.push(announce(if i % 2 == 0 {
            FwMagic::AnnounceLong
        } else {
            FwMagic::Announce
        }));
    }
    // 2. settle: short word only, covering the last application's 500 ms reboot delay.
    for _ in 0..(SETTLE_MS / period) {
        steps.push(announce(FwMagic::Announce));
    }
    steps
}

/// Phases 3-5: erase, data, run. Everything here writes flash on every board in earshot.
///
/// `image` must already have been through [`prepare_image`]. `base` is only used to size
/// the bank the image has to fit; the frames themselves carry offsets, not addresses, and
/// where those offsets land is decided entirely by the receiver. In practice a caller of
/// *this* path passes [`layout::APP_BASE_LEGACY`], because `"ER"` erases the legacy bank
/// on every bootloader that understands the word.
pub fn upload_steps(
    image: &[u8],
    base: u32,
    params: &FwUpdateParams,
) -> Result<Vec<FwStep>, FwUpdateError> {
    validate(image, base, params)?;

    let period = params.announce_period_ms.max(1);
    let announce = |magic| FwStep::Magic { magic, wait_ms: period };
    let mut steps = Vec::new();

    // 3. erase, twice, each covered by short announces.
    for _ in 0..ERASE_PASSES {
        steps.push(announce(FwMagic::Erase));
        for _ in 0..(ERASE_COVER_MS / period) {
            steps.push(announce(FwMagic::Announce));
        }
    }

    // 4. data frames. The offset strides by exactly the payload length -- the bug that
    //    broke the C++ Router was striding by a configured frame size while sending a
    //    hardcoded one, which trips the bootloader's continuity check on frame two.
    let mut offset = 0usize;
    while offset < image.len() {
        let end = (offset + params.frame_size).min(image.len());
        for _ in 0..params.frame_repetitions.max(1) {
            steps.push(FwStep::Data {
                offset: offset as u32,
                len: end - offset,
                wait_ms: params.wait_between_frames_ms,
            });
        }
        offset = end;
    }

    // 5. run.
    if params.run_after {
        steps.push(announce(FwMagic::Run));
    }
    Ok(steps)
}

/// Roughly how long a planned sequence occupies the bus.
///
/// The pacing gaps dominate, but not by enough to ignore the frames themselves: at
/// 115200 8N1 a 32-byte firmware frame is about 50 bytes on the wire, or 4.3 ms, which is
/// most of a 5 ms gap. Used to size the repeater's quiet window and to tell an operator
/// what they are waiting for, so it errs long rather than short.
pub fn estimate_duration_ms(steps: &[FwStep]) -> u64 {
    /// One bit each for the start and stop bits, on top of the eight data bits.
    const BITS_PER_BYTE: u64 = 10;
    const BAUD: u64 = router_proto::constants::BAUD_RATE as u64;
    steps
        .iter()
        .map(|step| {
            let wire_bytes = match *step {
                // envelope + fixmap + a uint32 key + bin8 header + checksum, then COBS and
                // its delimiter -- rounded up, because this is a budget not a measurement.
                FwStep::Data { len, .. } => len as u64 + 18,
                FwStep::Magic { .. } => 16,
            };
            u64::from(step.wait_ms()) + wire_bytes * BITS_PER_BYTE * 1_000 / BAUD
        })
        .sum()
}

/// A broadcast magic word, shaped so neither the worker nor the outbox can interfere.
///
/// `needs_ack: false` because nothing answers these, and `address: ""` with
/// `collateable: false` because the outbox keeps only the newest packet per non-empty
/// address -- which on a sequence of thousands of frames sharing one address would
/// deliver exactly the last one.
pub fn magic_packet(magic: FwMagic, wait_ms: u32) -> Packet {
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

/// The packet one planned step becomes. `image` is the prepared image the step indexes
/// into, so a `Data` step is rendered from the same bytes that were planned.
pub fn step_packet(step: FwStep, image: &[u8]) -> Packet {
    match step {
        FwStep::Magic { magic, wait_ms } => magic_packet(magic, wait_ms),
        FwStep::Data {
            offset,
            len,
            wait_ms,
        } => {
            let start = offset as usize;
            Packet {
                payload: Payload::Rendered(fw_frame_envelope(offset, &image[start..start + len])),
                target: -1,
                address: String::new(),
                needs_ack: false,
                collateable: false,
                custom_wait_time_ms: Some(wait_ms),
                on_sent: None,
            }
        }
    }
}

/// Enqueue the full upload sequence. Returns the number of packets queued.
pub fn upload(
    rs485: &Rs485,
    firmware: &[u8],
    base: u32,
    params: &FwUpdateParams,
) -> Result<usize, FwUpdateError> {
    let image = prepare_image(firmware, params);
    let steps = plan_for_image(&image, base, params)?;

    rs485.clear_outbox();
    for step in &steps {
        rs485.transmit(step_packet(*step, &image));
    }
    Ok(steps.len())
}

/// Broadcast the "erase" magic word once.
pub fn erase(rs485: &Rs485, params: &FwUpdateParams) {
    rs485.transmit(magic_packet(FwMagic::Erase, params.announce_period_ms));
}

/// Broadcast the "run application" magic word once.
pub fn run_application(rs485: &Rs485, params: &FwUpdateParams) {
    rs485.transmit(magic_packet(FwMagic::Run, params.announce_period_ms));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bank this blind path always writes: `"ER"` is only understood by a bootloader
    /// whose application starts here.
    const LEGACY: u32 = layout::APP_BASE_LEGACY;
    const LEGACY_BANK: usize = 100_352;

    fn data_steps(steps: &[FwStep]) -> Vec<(u32, usize)> {
        steps
            .iter()
            .filter_map(|s| match *s {
                FwStep::Data { offset, len, .. } => Some((offset, len)),
                _ => None,
            })
            .collect()
    }

    fn magics(steps: &[FwStep]) -> Vec<FwMagic> {
        steps
            .iter()
            .filter_map(|s| match *s {
                FwStep::Magic { magic, .. } => Some(magic),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn persistent_pages_are_outside_every_accepted_upload() {
        let params = FwUpdateParams::default();
        assert_eq!(layout::app_bank_bytes(LEGACY), LEGACY_BANK);
        assert!(plan(&vec![0x00; LEGACY_BANK], LEGACY, &params).is_ok());
        assert!(matches!(
            plan(&vec![0x00; LEGACY_BANK + 1], LEGACY, &params),
            Err(FwUpdateError::TooLarge { .. })
        ));
    }

    /// The limit follows the target's bank, not this module's opinion. An image that a v6
    /// board takes happily is 8 kB too large for a v4/v5 one, and the 8 kB it overruns by
    /// is the provisioning serial and the settings journals.
    #[test]
    fn the_size_limit_is_the_receiving_bank_s() {
        let params = FwUpdateParams::default();
        let new_bank = layout::app_bank_bytes(layout::APP_BASE);
        assert_eq!(new_bank - LEGACY_BANK, 8_192);
        assert!(plan(&vec![0x00; new_bank], layout::APP_BASE, &params).is_ok());
        assert_eq!(
            plan(&vec![0x00; new_bank], LEGACY, &params),
            Err(FwUpdateError::TooLarge {
                bytes: new_bank,
                limit: LEGACY_BANK,
                base: LEGACY,
            })
        );
        assert_eq!(
            plan(&[0u8; 64], layout::FLASH_BASE, &params),
            Err(FwUpdateError::NotAnApplicationBase {
                base: layout::FLASH_BASE
            })
        );
    }

    /// The real production image: 98,196 bytes, no trailing 0xFF to truncate, and four
    /// bytes short of a double-word. Left unpadded, the fielded bootloader programs four
    /// bytes of its own stack into the end of the application bank.
    #[test]
    fn the_production_image_length_is_padded_up_to_a_double_word() {
        let params = FwUpdateParams::default();
        let image = prepare_image(&[0xA5; 98_196], &params);
        assert_eq!(image.len(), 98_200);
        assert!(image.len().is_multiple_of(FLASH_WRITE_GRANULE));
        assert_eq!(&image[98_196..], &[0xFF; 4]);
    }

    #[test]
    fn truncation_still_happens_and_the_result_is_still_padded() {
        let params = FwUpdateParams::default();
        let mut firmware = vec![0x11; 100];
        firmware.extend_from_slice(&[0xFF; 500]);
        let image = prepare_image(&firmware, &params);
        assert_eq!(image.len(), 104, "truncated to 100, padded back to 104");
        assert_eq!(&image[..100], &[0x11; 100]);
        assert_eq!(&image[100..], &[0xFF; 4]);
    }

    #[test]
    fn padding_never_pushes_a_maximal_image_over_the_bank() {
        let params = FwUpdateParams { truncate: false, ..Default::default() };
        // The bank is itself a multiple of 8, so a full image needs no padding at all.
        assert!(LEGACY_BANK.is_multiple_of(FLASH_WRITE_GRANULE));
        let image = prepare_image(&vec![0x00; LEGACY_BANK], &params);
        assert_eq!(image.len(), LEGACY_BANK);
        assert!(validate(&image, LEGACY, &params).is_ok());
    }

    /// The bootloader only accepts strictly increasing, gapless offsets: `frameOffset`
    /// below its write position is skipped, above it kills the upload outright.
    #[test]
    fn offsets_stride_by_exactly_the_payload_length() {
        let params = FwUpdateParams { frame_size: 32, ..Default::default() };
        let steps = plan(&[0x5A; 98_196], LEGACY, &params).unwrap();
        let data = data_steps(&steps);
        assert_eq!(data.len(), 98_200 / 32 + 1); // 3068 full frames + a 24-byte tail
        let mut expected = 0u32;
        for (offset, len) in &data {
            assert_eq!(*offset, expected, "offsets must be gapless and in order");
            expected += *len as u32;
        }
        assert_eq!(expected as usize, 98_200);
        assert!(data.last().unwrap().1.is_multiple_of(FLASH_WRITE_GRANULE));
    }

    #[test]
    fn repetitions_repeat_an_offset_in_place_rather_than_replaying_the_image() {
        let params = FwUpdateParams { frame_size: 32, frame_repetitions: 3, ..Default::default() };
        let steps = plan(&[0u8; 96], LEGACY, &params).unwrap();
        let data = data_steps(&steps);
        assert_eq!(
            data,
            vec![(0, 32), (0, 32), (0, 32), (32, 32), (32, 32), (32, 32), (64, 32), (64, 32), (64, 32)]
        );
    }

    /// The defect this module existed to hide: a bootloader hears nothing it can parse
    /// for the whole announce, times out after 3 s, and falls back into the application.
    #[test]
    fn the_announce_never_leaves_a_bootloader_without_a_word_it_can_parse() {
        let params = FwUpdateParams::default();
        let steps = plan(&[0u8; 64], LEGACY, &params).unwrap();

        // Residency floor is 3 s from reset; anything an accepted frame does not reach
        // within that window drops back into the application.
        const RESIDENCY_FLOOR_MS: u32 = 3_000;
        let mut since_parseable = 0u32;
        let mut worst = 0u32;
        for step in &steps {
            let parseable = !matches!(
                step,
                FwStep::Magic { magic: FwMagic::AnnounceLong, .. }
            );
            if parseable {
                since_parseable = 0;
            } else {
                since_parseable += step.wait_ms();
                worst = worst.max(since_parseable);
            }
        }
        assert!(
            worst < RESIDENCY_FLOOR_MS,
            "a bootloader would go {worst} ms without a parseable word (floor {RESIDENCY_FLOOR_MS} ms)"
        );
    }

    /// ...and the mirror of it: an application only reboots on the long word, so the
    /// announce has to keep sending one until every board has had time to reset.
    #[test]
    fn the_announce_keeps_bumping_applications_for_the_whole_bump_phase() {
        let params = FwUpdateParams::default();
        let steps = plan(&[0u8; 64], LEGACY, &params).unwrap();
        let long: Vec<usize> = steps
            .iter()
            .enumerate()
            .filter(|(_, s)| matches!(s, FwStep::Magic { magic: FwMagic::AnnounceLong, .. }))
            .map(|(i, _)| i)
            .collect();
        assert!(long.len() >= 20, "only {} long announces", long.len());

        // Every long announce is followed within one period by another one or by the end
        // of the bump phase -- an application that misses one gets the next.
        let elapsed_to_last_long = long.last().unwrap() * params.announce_period_ms as usize;
        assert!(
            elapsed_to_last_long as u32 >= BUMP_MS - params.announce_period_ms * 2,
            "the bump phase stops bumping too early"
        );

        // The application's own reboot takes 500 ms, and the erase must not happen until
        // the last board to hear a long word is resident in its bootloader.
        let first_erase = steps
            .iter()
            .position(|s| matches!(s, FwStep::Magic { magic: FwMagic::Erase, .. }))
            .expect("an erase");
        let gap_ms = (first_erase - long.last().unwrap()) as u32 * params.announce_period_ms;
        assert!(
            gap_ms >= 1_000,
            "only {gap_ms} ms between the last reboot word and the erase; a board \
             that reset 500 ms late would miss it"
        );
    }

    #[test]
    fn every_erase_is_followed_by_announces_that_reset_the_write_position() {
        let params = FwUpdateParams::default();
        let steps = plan(&[0u8; 64], LEGACY, &params).unwrap();
        let words = magics(&steps);
        let erases: Vec<usize> = words
            .iter()
            .enumerate()
            .filter(|(_, m)| **m == FwMagic::Erase)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(erases.len(), ERASE_PASSES);
        for erase_at in erases {
            let cover = words[erase_at + 1..]
                .iter()
                .take_while(|m| **m == FwMagic::Announce)
                .count() as u32;
            assert!(
                cover * params.announce_period_ms >= ERASE_COVER_MS,
                "an erase is covered by only {cover} announces"
            );
        }
    }

    #[test]
    fn the_data_phase_starts_only_after_the_last_erase() {
        let params = FwUpdateParams::default();
        let steps = plan(&[0u8; 64], LEGACY, &params).unwrap();
        let last_erase = steps
            .iter()
            .rposition(|s| matches!(s, FwStep::Magic { magic: FwMagic::Erase, .. }))
            .unwrap();
        let first_data = steps
            .iter()
            .position(|s| matches!(s, FwStep::Data { .. }))
            .unwrap();
        assert!(last_erase < first_data);
    }

    #[test]
    fn run_after_puts_the_handoff_at_the_very_end() {
        let params = FwUpdateParams { run_after: true, ..Default::default() };
        let steps = plan(&[0u8; 64], LEGACY, &params).unwrap();
        assert!(matches!(
            steps.last().unwrap(),
            FwStep::Magic { magic: FwMagic::Run, .. }
        ));
        assert_eq!(
            magics(&steps).iter().filter(|m| **m == FwMagic::Run).count(),
            1
        );
        // ...and stays off by default, because the GUI drives it from its own button.
        let steps = plan(&[0u8; 64], LEGACY, &FwUpdateParams::default()).unwrap();
        assert!(!magics(&steps).contains(&FwMagic::Run));
    }

    /// The shape of a real run, so a change to the announce phases shows up as a number
    /// somebody has to look at rather than as a slower bench session.
    #[test]
    fn the_production_image_plans_a_sequence_of_known_size_and_duration() {
        let params = FwUpdateParams { run_after: true, ..FwUpdateParams::resilient() };
        let steps = plan(&[0xA5; 98_196], LEGACY, &params).unwrap();

        let announces = magics(&steps).len();
        let frames = data_steps(&steps).len();
        assert_eq!(announces, 133, "132 announce/erase packets, plus the trailing RU");
        assert_eq!(frames, 3_069 * 2, "3,069 frames of a 98,200-byte padded image, x2");

        let seconds = estimate_duration_ms(&steps) / 1_000;
        assert!(
            (95..=130).contains(&seconds),
            "a resilient-profile run of the production image estimated at {seconds}s"
        );
    }

    #[test]
    fn an_odd_frame_size_is_refused_rather_than_silently_mismatching_the_checksum() {
        let params = FwUpdateParams { frame_size: 33, ..Default::default() };
        assert!(matches!(
            plan(&[0u8; 64], LEGACY, &params),
            Err(FwUpdateError::OddFrameSize)
        ));
        let params = FwUpdateParams { frame_size: 0, ..Default::default() };
        assert!(matches!(
            plan(&[0u8; 64], LEGACY, &params),
            Err(FwUpdateError::ZeroFrameSize)
        ));
    }
}
