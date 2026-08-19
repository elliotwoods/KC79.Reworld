//! Durable provisioning identity and module settings stored in the final three flash pages.
//!
//! The byte layout is deliberately hand-encoded. Rust struct layout is not a storage format,
//! and the firmware reads these same bytes from C++. A committed golden vector below makes any
//! accidental cross-language change visible.

use serde::{Deserialize, Serialize};

use crate::addr;

pub const RECORD_BYTES: usize = 64;
pub const RECORDS_PER_PAGE: usize = addr::FLASH_PAGE_BYTES as usize / RECORD_BYTES;
pub const MAGIC: u64 = u64::from_le_bytes(*b"KCPRV001");
pub const SCHEMA_VERSION: u16 = 1;
const KIND_IDENTITY: u16 = 1;
const KIND_SETTINGS: u16 = 2;
const PAYLOAD_BYTES: usize = 28;
const CRC_OFFSET: usize = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct McuUid(pub [u32; 3]);

impl McuUid {
    pub fn hex(self) -> String {
        format!("{:08X}{:08X}{:08X}", self.0[0], self.0[1], self.0[2])
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityRecord {
    pub generation: u32,
    pub uid: McuUid,
    pub serial: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum IdentityState {
    Blank,
    Valid { record: IdentityRecord },
    Corrupt,
    ForeignUid { record: IdentityRecord },
}

impl IdentityState {
    pub fn serial(self) -> Option<u32> {
        match self {
            Self::Valid { record } => Some(record.serial),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Blank => "blank",
            Self::Valid { .. } => "valid",
            Self::Corrupt => "corrupt",
            Self::ForeignUid { .. } => "foreign-uid",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceSettings {
    pub operating_current_ma: u16,
    pub full_current_home_recovery: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub axis_a_calibration: Option<OpticalCalibration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub axis_b_calibration: Option<OpticalCalibration>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpticalCalibration {
    pub algorithm_version: u16,
    pub threshold: u8,
    pub width_usteps: u32,
}

impl OpticalCalibration {
    fn validate(self) -> bool {
        self.algorithm_version > 0
            && self.threshold >= 16
            && (8..=4_200).contains(&self.width_usteps)
    }
}

impl Default for DeviceSettings {
    fn default() -> Self {
        Self {
            operating_current_ma: 150,
            full_current_home_recovery: true,
            axis_a_calibration: None,
            axis_b_calibration: None,
        }
    }
}

impl DeviceSettings {
    pub fn validate(self) -> bool {
        (50..=250).contains(&self.operating_current_ma)
            && self
                .axis_a_calibration
                .is_none_or(OpticalCalibration::validate)
            && self
                .axis_b_calibration
                .is_none_or(OpticalCalibration::validate)
            && self
                .axis_a_calibration
                .zip(self.axis_b_calibration)
                .is_none_or(|(a, b)| a.algorithm_version == b.algorithm_version)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsRecord {
    pub generation: u32,
    pub uid: McuUid,
    pub settings: DeviceSettings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SettingsSource {
    Defaults,
    FlashA,
    FlashB,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsState {
    pub record: SettingsRecord,
    pub source: SettingsSource,
    pub corrupt_records: u32,
}

impl SettingsState {
    pub fn load(page_a: &[u8], page_b: &[u8], uid: McuUid) -> Self {
        let (a, bad_a) = scan_settings_page(page_a, uid);
        let (b, bad_b) = scan_settings_page(page_b, uid);
        let (record, source) = match (a, b) {
            (Some(a), Some(b)) if b.generation > a.generation => (b, SettingsSource::FlashB),
            (Some(a), Some(_)) | (Some(a), None) => (a, SettingsSource::FlashA),
            (None, Some(b)) => (b, SettingsSource::FlashB),
            (None, None) => (
                SettingsRecord {
                    generation: 0,
                    uid,
                    settings: DeviceSettings::default(),
                },
                SettingsSource::Defaults,
            ),
        };
        Self {
            record,
            source,
            corrupt_records: bad_a + bad_b,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JournalWrite {
    /// Program one erased 64-byte slot without erasing its page.
    Append { address: u32 },
    /// Erase this page, then program its first slot. The other page remains the committed copy
    /// until the new record has been read back successfully.
    Compact { page_address: u32 },
}

pub fn scan_identity_page(page: &[u8], uid: McuUid) -> IdentityState {
    let mut local = None;
    let mut foreign = None;
    let mut corrupt = false;
    for bytes in records(page) {
        if erased(bytes) {
            continue;
        }
        match decode_identity(bytes) {
            Some(record) if record.uid == uid => keep_latest(&mut local, record),
            Some(record) => keep_latest(&mut foreign, record),
            None => corrupt = true,
        }
    }
    if let Some(record) = local {
        IdentityState::Valid { record }
    } else if let Some(record) = foreign {
        IdentityState::ForeignUid { record }
    } else if corrupt {
        IdentityState::Corrupt
    } else {
        IdentityState::Blank
    }
}

pub fn identity_write(page: &[u8]) -> Option<JournalWrite> {
    first_erased_slot(page).map(|slot| JournalWrite::Append {
        address: addr::IDENTITY_BASE + (slot * RECORD_BYTES) as u32,
    })
}

pub fn settings_write(
    page_a: &[u8],
    page_b: &[u8],
    current_source: SettingsSource,
) -> JournalWrite {
    let (active_page, active_bytes, inactive_page) = match current_source {
        SettingsSource::FlashB => (addr::SETTINGS_B_BASE, page_b, addr::SETTINGS_A_BASE),
        SettingsSource::Defaults | SettingsSource::FlashA => {
            (addr::SETTINGS_A_BASE, page_a, addr::SETTINGS_B_BASE)
        }
    };
    match first_erased_slot(active_bytes) {
        Some(slot) => JournalWrite::Append {
            address: active_page + (slot * RECORD_BYTES) as u32,
        },
        None => JournalWrite::Compact {
            page_address: inactive_page,
        },
    }
}

pub fn encode_identity(record: IdentityRecord) -> [u8; RECORD_BYTES] {
    let mut payload = [0xFF; PAYLOAD_BYTES];
    payload[..4].copy_from_slice(&record.serial.to_le_bytes());
    encode(KIND_IDENTITY, record.generation, record.uid, 4, payload)
}

pub fn decode_identity(bytes: &[u8]) -> Option<IdentityRecord> {
    let decoded = decode(bytes, KIND_IDENTITY, 4)?;
    let serial = u32::from_le_bytes(decoded.payload[..4].try_into().ok()?);
    if serial == 0 || serial == u32::MAX {
        return None;
    }
    Some(IdentityRecord {
        generation: decoded.generation,
        uid: decoded.uid,
        serial,
    })
}

pub fn encode_settings(record: SettingsRecord) -> [u8; RECORD_BYTES] {
    let mut payload = [0xFF; PAYLOAD_BYTES];
    payload[..2].copy_from_slice(&record.settings.operating_current_ma.to_le_bytes());
    payload[2] = u8::from(record.settings.full_current_home_recovery);
    payload[3] = 2;
    let algorithm_version = record
        .settings
        .axis_a_calibration
        .or(record.settings.axis_b_calibration)
        .map_or(0, |calibration| calibration.algorithm_version);
    payload[4..6].copy_from_slice(&algorithm_version.to_le_bytes());
    payload[6] = u8::from(record.settings.axis_a_calibration.is_some())
        | (u8::from(record.settings.axis_b_calibration.is_some()) << 1);
    if let Some(calibration) = record.settings.axis_a_calibration {
        payload[7] = calibration.threshold;
        payload[8..12].copy_from_slice(&calibration.width_usteps.to_le_bytes());
    }
    if let Some(calibration) = record.settings.axis_b_calibration {
        payload[12] = calibration.threshold;
        payload[13..17].copy_from_slice(&calibration.width_usteps.to_le_bytes());
    }
    encode(KIND_SETTINGS, record.generation, record.uid, 17, payload)
}

pub fn decode_settings(bytes: &[u8]) -> Option<SettingsRecord> {
    let decoded = decode(bytes, KIND_SETTINGS, 0)?;
    if decoded.payload_len != 3 && decoded.payload_len != 17 {
        return None;
    }
    let mut settings = DeviceSettings {
        operating_current_ma: u16::from_le_bytes(decoded.payload[..2].try_into().ok()?),
        full_current_home_recovery: match decoded.payload[2] {
            0 => false,
            1 => true,
            _ => return None,
        },
        axis_a_calibration: None,
        axis_b_calibration: None,
    };
    if decoded.payload_len == 17 {
        if decoded.payload[3] != 2 || decoded.payload[6] & !3 != 0 {
            return None;
        }
        let algorithm_version = u16::from_le_bytes(decoded.payload[4..6].try_into().ok()?);
        if decoded.payload[6] & 1 != 0 {
            settings.axis_a_calibration = Some(OpticalCalibration {
                algorithm_version,
                threshold: decoded.payload[7],
                width_usteps: u32::from_le_bytes(decoded.payload[8..12].try_into().ok()?),
            });
        }
        if decoded.payload[6] & 2 != 0 {
            settings.axis_b_calibration = Some(OpticalCalibration {
                algorithm_version,
                threshold: decoded.payload[12],
                width_usteps: u32::from_le_bytes(decoded.payload[13..17].try_into().ok()?),
            });
        }
    }
    if !settings.validate() {
        return None;
    }
    Some(SettingsRecord {
        generation: decoded.generation,
        uid: decoded.uid,
        settings,
    })
}

struct Decoded {
    generation: u32,
    uid: McuUid,
    payload_len: u32,
    payload: [u8; PAYLOAD_BYTES],
}

fn encode(
    kind: u16,
    generation: u32,
    uid: McuUid,
    payload_len: u32,
    payload: [u8; PAYLOAD_BYTES],
) -> [u8; RECORD_BYTES] {
    let mut out = [0xFF; RECORD_BYTES];
    out[0..8].copy_from_slice(&MAGIC.to_le_bytes());
    out[8..10].copy_from_slice(&SCHEMA_VERSION.to_le_bytes());
    out[10..12].copy_from_slice(&kind.to_le_bytes());
    out[12..16].copy_from_slice(&generation.to_le_bytes());
    out[16..20].copy_from_slice(&payload_len.to_le_bytes());
    for (index, word) in uid.0.iter().enumerate() {
        out[20 + index * 4..24 + index * 4].copy_from_slice(&word.to_le_bytes());
    }
    out[32..60].copy_from_slice(&payload);
    let crc = crc32c(&out[..CRC_OFFSET]);
    out[CRC_OFFSET..].copy_from_slice(&crc.to_le_bytes());
    out
}

fn decode(bytes: &[u8], expected_kind: u16, expected_payload: u32) -> Option<Decoded> {
    if bytes.len() != RECORD_BYTES || erased(bytes) {
        return None;
    }
    let u16_at = |at| u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap());
    let u32_at = |at| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let u64_at = |at| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
    if u64_at(0) != MAGIC
        || u16_at(8) != SCHEMA_VERSION
        || u16_at(10) != expected_kind
        || (expected_payload != 0 && u32_at(16) != expected_payload)
        || u32_at(CRC_OFFSET) != crc32c(&bytes[..CRC_OFFSET])
    {
        return None;
    }
    let mut payload = [0; PAYLOAD_BYTES];
    payload.copy_from_slice(&bytes[32..60]);
    Some(Decoded {
        generation: u32_at(12),
        uid: McuUid([u32_at(20), u32_at(24), u32_at(28)]),
        payload_len: u32_at(16),
        payload,
    })
}

fn scan_settings_page(page: &[u8], uid: McuUid) -> (Option<SettingsRecord>, u32) {
    let mut latest = None;
    let mut corrupt = 0;
    for bytes in records(page) {
        if erased(bytes) {
            continue;
        }
        match decode_settings(bytes) {
            Some(record) if record.uid == uid => keep_latest(&mut latest, record),
            Some(_) => {}
            None => corrupt += 1,
        }
    }
    (latest, corrupt)
}

fn records(page: &[u8]) -> impl Iterator<Item = &[u8]> {
    page.chunks_exact(RECORD_BYTES).take(RECORDS_PER_PAGE)
}

fn first_erased_slot(page: &[u8]) -> Option<usize> {
    records(page).position(erased)
}

fn erased(bytes: &[u8]) -> bool {
    bytes.iter().all(|byte| *byte == 0xFF)
}

trait Generation {
    fn generation(&self) -> u32;
}
impl Generation for IdentityRecord {
    fn generation(&self) -> u32 {
        self.generation
    }
}
impl Generation for SettingsRecord {
    fn generation(&self) -> u32 {
        self.generation
    }
}
fn keep_latest<T: Copy + Generation>(slot: &mut Option<T>, candidate: T) {
    if slot.is_none_or(|old| candidate.generation() > old.generation()) {
        *slot = Some(candidate);
    }
}

/// CRC-32C (Castagnoli), reflected form. Small and dependency-free so the exact algorithm can
/// be copied into the embedded build and tested against the standard `123456789` vector.
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

    const UID: McuUid = McuUid([0x1122_3344, 0x5566_7788, 0x99AA_BBCC]);

    fn blank() -> Vec<u8> {
        vec![0xFF; addr::FLASH_PAGE_BYTES as usize]
    }

    #[test]
    fn crc32c_standard_vector() {
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
    }

    #[test]
    fn identity_rejects_blank_random_torn_foreign_and_bad_ranges() {
        let mut page = blank();
        assert_eq!(scan_identity_page(&page, UID), IdentityState::Blank);

        page[0..8].copy_from_slice(b"random!!");
        assert_eq!(scan_identity_page(&page, UID), IdentityState::Corrupt);

        page = blank();
        let record = IdentityRecord {
            generation: 7,
            uid: UID,
            serial: 42,
        };
        page[..RECORD_BYTES].copy_from_slice(&encode_identity(record));
        page[CRC_OFFSET] ^= 1;
        assert_eq!(scan_identity_page(&page, UID), IdentityState::Corrupt);

        page = blank();
        let foreign = IdentityRecord {
            uid: McuUid([1, 2, 3]),
            ..record
        };
        page[..RECORD_BYTES].copy_from_slice(&encode_identity(foreign));
        assert_eq!(
            scan_identity_page(&page, UID),
            IdentityState::ForeignUid { record: foreign }
        );

        for serial in [0, u32::MAX] {
            page = blank();
            page[..RECORD_BYTES]
                .copy_from_slice(&encode_identity(IdentityRecord { serial, ..record }));
            assert_eq!(scan_identity_page(&page, UID), IdentityState::Corrupt);
        }
    }

    #[test]
    fn identity_is_latest_generation_and_append_only() {
        let mut page = blank();
        for (slot, generation, serial) in [(0, 4, 40), (1, 9, 90), (2, 6, 60)] {
            let bytes = encode_identity(IdentityRecord {
                generation,
                uid: UID,
                serial,
            });
            let at = slot * RECORD_BYTES;
            page[at..at + RECORD_BYTES].copy_from_slice(&bytes);
        }
        assert_eq!(scan_identity_page(&page, UID).serial(), Some(90));
        assert_eq!(
            identity_write(&page),
            Some(JournalWrite::Append {
                address: addr::IDENTITY_BASE + 3 * RECORD_BYTES as u32
            })
        );
        page.fill(0);
        assert_eq!(identity_write(&page), None);
    }

    #[test]
    fn settings_select_latest_across_pages_and_roll_over_safely() {
        let mut a = blank();
        let mut b = blank();
        let first = SettingsRecord {
            generation: 2,
            uid: UID,
            settings: DeviceSettings::default(),
        };
        let promoted = SettingsRecord {
            generation: 3,
            uid: UID,
            settings: DeviceSettings {
                operating_current_ma: 250,
                full_current_home_recovery: true,
                ..DeviceSettings::default()
            },
        };
        a[..RECORD_BYTES].copy_from_slice(&encode_settings(first));
        b[..RECORD_BYTES].copy_from_slice(&encode_settings(promoted));
        let state = SettingsState::load(&a, &b, UID);
        assert_eq!(state.record, promoted);
        assert_eq!(state.source, SettingsSource::FlashB);
        assert_eq!(
            settings_write(&a, &b, state.source),
            JournalWrite::Append {
                address: addr::SETTINGS_B_BASE + RECORD_BYTES as u32
            }
        );

        b.fill(0);
        assert_eq!(
            settings_write(&a, &b, SettingsSource::FlashB),
            JournalWrite::Compact {
                page_address: addr::SETTINGS_A_BASE
            }
        );
    }

    #[test]
    fn exact_golden_identity_layout() {
        let bytes = encode_identity(IdentityRecord {
            generation: 0x0102_0304,
            uid: UID,
            serial: 123_456,
        });
        let actual = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        assert_eq!(
            actual,
            "4b435052563030310100010004030201040000004433221188776655ccbbaa9940e20100ffffffffffffffffffffffffffffffffffffffffffffffff2758ad80"
        );
    }

    #[test]
    fn settings_v2_round_trips_calibration_and_has_a_fixed_cross_language_layout() {
        let record = SettingsRecord {
            generation: 5,
            uid: UID,
            settings: DeviceSettings {
                operating_current_ma: 150,
                full_current_home_recovery: true,
                axis_a_calibration: Some(OpticalCalibration {
                    algorithm_version: 1,
                    threshold: 235,
                    width_usteps: 871,
                }),
                axis_b_calibration: Some(OpticalCalibration {
                    algorithm_version: 1,
                    threshold: 235,
                    width_usteps: 407,
                }),
            },
        };
        let bytes = encode_settings(record);
        assert_eq!(decode_settings(&bytes), Some(record));
        let actual = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        assert_eq!(
            actual,
            "4b435052563030310100020005000000110000004433221188776655ccbbaa9996000102010003eb67030000eb97010000ffffffffffffffffffffff334addb2"
        );
    }

    #[test]
    fn legacy_settings_remain_readable_and_torn_v2_is_rejected() {
        let mut payload = [0xFF; PAYLOAD_BYTES];
        payload[..2].copy_from_slice(&150u16.to_le_bytes());
        payload[2] = 1;
        let legacy = encode(KIND_SETTINGS, 2, UID, 3, payload);
        assert_eq!(
            decode_settings(&legacy).unwrap().settings,
            DeviceSettings::default()
        );

        let mut torn = encode_settings(SettingsRecord {
            generation: 3,
            uid: UID,
            settings: DeviceSettings::default(),
        });
        torn[44] ^= 1;
        assert_eq!(decode_settings(&torn), None);
    }
}
