#include "PersistentStorage.h"
#include "Logger.h"

#include <Arduino.h>
#include <string.h>

namespace {
	static constexpr uint64_t Magic = 0x313030565250434BULL; // little-endian "KCPRV001"
	static constexpr uint16_t Schema = 1;
	static constexpr uint16_t IdentityKind = 1;
	static constexpr uint16_t SettingsKind = 2;
	static constexpr uint8_t SettingsPayloadVersion = 2;
	static constexpr uint32_t SettingsPayloadV1Bytes = 3;
	static constexpr uint32_t SettingsPayloadV2Bytes = 17;
	static constexpr uint32_t UIDAddress = 0x1FFF7590U;
	static constexpr uint32_t RecordsPerPage = PersistentStorage::PageBytes / PersistentStorage::RecordBytes;

	uint16_t get16(const uint8_t * bytes, uint32_t at) {
		return (uint16_t) bytes[at] | ((uint16_t) bytes[at + 1] << 8);
	}
	uint32_t get32(const uint8_t * bytes, uint32_t at) {
		return (uint32_t) bytes[at] | ((uint32_t) bytes[at + 1] << 8)
			| ((uint32_t) bytes[at + 2] << 16) | ((uint32_t) bytes[at + 3] << 24);
	}
	uint64_t get64(const uint8_t * bytes, uint32_t at) {
		return (uint64_t) get32(bytes, at) | ((uint64_t) get32(bytes, at + 4) << 32);
	}
	void put16(uint8_t * bytes, uint32_t at, uint16_t value) {
		bytes[at] = (uint8_t) value; bytes[at + 1] = (uint8_t) (value >> 8);
	}
	void put32(uint8_t * bytes, uint32_t at, uint32_t value) {
		for(uint8_t i = 0; i < 4; i++) bytes[at + i] = (uint8_t) (value >> (8U * i));
	}
	void put64(uint8_t * bytes, uint32_t at, uint64_t value) {
		put32(bytes, at, (uint32_t) value); put32(bytes, at + 4, (uint32_t) (value >> 32));
	}

	uint32_t crc32c(const uint8_t * bytes, uint32_t count) {
		uint32_t crc = 0xFFFFFFFFU;
		for(uint32_t i = 0; i < count; i++) {
			crc ^= bytes[i];
			for(uint8_t bit = 0; bit < 8; bit++) {
				crc = (crc >> 1) ^ ((crc & 1U) ? 0x82F63B78U : 0U);
			}
		}
		return ~crc;
	}

	bool erased(const uint8_t * bytes) {
		for(uint32_t i = 0; i < PersistentStorage::RecordBytes; i++) if(bytes[i] != 0xFF) return false;
		return true;
	}

	void uid(uint32_t out[3]) {
		const volatile uint32_t * words = (const volatile uint32_t *) UIDAddress;
		out[0] = words[0]; out[1] = words[1]; out[2] = words[2];
	}

	bool header(const uint8_t * bytes, uint16_t kind, uint32_t payloadLength, uint32_t uidWords[3]) {
		if(erased(bytes) || get64(bytes, 0) != Magic || get16(bytes, 8) != Schema
			|| get16(bytes, 10) != kind
			|| (payloadLength != 0 && get32(bytes, 16) != payloadLength)
			|| get32(bytes, 60) != crc32c(bytes, 60)) return false;
		uidWords[0] = get32(bytes, 20); uidWords[1] = get32(bytes, 24); uidWords[2] = get32(bytes, 28);
		return true;
	}

	bool sameUID(const uint32_t a[3], const uint32_t b[3]) {
		return a[0] == b[0] && a[1] == b[1] && a[2] == b[2];
	}

	bool decodeSettings(const uint8_t * bytes, PersistentStorage::Settings& value, bool& foreign) {
		uint32_t storedUID[3], ownUID[3]; uid(ownUID);
		if(!header(bytes, SettingsKind, 0, storedUID)) return false;
		const uint32_t payloadLength = get32(bytes, 16);
		if(payloadLength != SettingsPayloadV1Bytes && payloadLength != SettingsPayloadV2Bytes) return false;
		foreign = !sameUID(storedUID, ownUID);
		const uint16_t current = get16(bytes, 32);
		const uint8_t recovery = bytes[34];
		if(current < 50 || current > 250 || recovery > 1) return false;
		value.generation = get32(bytes, 12);
		value.operatingCurrentMa = current;
		value.fullCurrentHomeRecovery = recovery != 0;
		if(payloadLength == SettingsPayloadV2Bytes) {
			if(bytes[35] != SettingsPayloadVersion) return false;
			value.opticalCalibrationVersion = get16(bytes, 36);
			const uint8_t validMask = bytes[38];
			if((validMask & ~3U) != 0) return false;
			value.axisACalibrationValid = (validMask & 1U) != 0;
			value.axisAThreshold = bytes[39];
			value.axisAWidth = (int32_t) get32(bytes, 40);
			value.axisBCalibrationValid = (validMask & 2U) != 0;
			value.axisBThreshold = bytes[44];
			value.axisBWidth = (int32_t) get32(bytes, 45);
			if((value.axisACalibrationValid && (value.axisAThreshold < 16
				|| value.axisAWidth < 8 || value.axisAWidth > 4200))
				|| (value.axisBCalibrationValid && (value.axisBThreshold < 16
				|| value.axisBWidth < 8 || value.axisBWidth > 4200))) return false;
		}
		value.valid = !foreign;
		return true;
	}

	int firstErased(uint32_t pageAddress) {
		for(uint32_t slot = 0; slot < RecordsPerPage; slot++) {
			if(erased((const uint8_t *)(pageAddress + slot * PersistentStorage::RecordBytes))) return (int) slot;
		}
		return -1;
	}

	void makeSettings(uint8_t out[PersistentStorage::RecordBytes], uint32_t generation,
		const PersistentStorage::Settings& value) {
		memset(out, 0xFF, PersistentStorage::RecordBytes);
		put64(out, 0, Magic); put16(out, 8, Schema); put16(out, 10, SettingsKind);
		put32(out, 12, generation); put32(out, 16, SettingsPayloadV2Bytes);
		uint32_t ownUID[3]; uid(ownUID);
		put32(out, 20, ownUID[0]); put32(out, 24, ownUID[1]); put32(out, 28, ownUID[2]);
		put16(out, 32, value.operatingCurrentMa);
		out[34] = value.fullCurrentHomeRecovery ? 1 : 0;
		out[35] = SettingsPayloadVersion;
		put16(out, 36, value.opticalCalibrationVersion);
		out[38] = (value.axisACalibrationValid ? 1U : 0U)
			| (value.axisBCalibrationValid ? 2U : 0U);
		out[39] = value.axisAThreshold;
		put32(out, 40, (uint32_t) value.axisAWidth);
		out[44] = value.axisBThreshold;
		put32(out, 45, (uint32_t) value.axisBWidth);
		put32(out, 60, crc32c(out, 60));
	}

	bool erasePage(uint32_t address) {
		FLASH_EraseInitTypeDef eraseInit = {};
		eraseInit.TypeErase = FLASH_TYPEERASE_PAGES;
		eraseInit.Banks = FLASH_BANK_1;
		eraseInit.Page = (address - FLASH_BASE) / FLASH_PAGE_SIZE;
		eraseInit.NbPages = 1;
		uint32_t pageError = 0;
		return HAL_FLASHEx_Erase(&eraseInit, &pageError) == HAL_OK;
	}

	bool programRecord(uint32_t address, const uint8_t bytes[PersistentStorage::RecordBytes]) {
		for(uint32_t at = 0; at < PersistentStorage::RecordBytes; at += 8) {
			uint64_t word; memcpy(&word, bytes + at, sizeof(word));
			if(HAL_FLASH_Program(FLASH_TYPEPROGRAM_DOUBLEWORD, address + at, word) != HAL_OK) {
				char message[110];
				sprintf(message, "program failed: address=0x%08lX error=0x%08lX SR=0x%08lX"
					, (unsigned long) (address + at), (unsigned long) HAL_FLASH_GetError()
					, (unsigned long) FLASH->SR);
				log(LogLevel::Error, "PersistentStorage", message);
				return false;
			}
		}
		const bool verified = memcmp((const void *) address, bytes, PersistentStorage::RecordBytes) == 0;
		if(!verified) log(LogLevel::Error, "PersistentStorage", "program readback mismatch");
		return verified;
	}
}

namespace PersistentStorage {
	Identity readIdentity() {
		Identity result;
		uint32_t ownUID[3]; uid(ownUID);
		for(uint32_t slot = 0; slot < RecordsPerPage; slot++) {
			const uint8_t * bytes = (const uint8_t *)(IdentityAddress + slot * RecordBytes);
			if(erased(bytes)) continue;
			uint32_t storedUID[3];
			if(!header(bytes, IdentityKind, 4, storedUID)) { result.corrupt = true; continue; }
			const uint32_t serial = get32(bytes, 32);
			if(serial == 0 || serial == 0xFFFFFFFFU) { result.corrupt = true; continue; }
			const uint32_t generation = get32(bytes, 12);
			if(!sameUID(storedUID, ownUID)) { result.foreignUID = true; continue; }
			if(!result.valid || generation > result.generation) {
				result.valid = true; result.generation = generation; result.serial = serial;
			}
		}
		return result;
	}

	Settings readSettings() {
		Settings best;
		for(uint8_t pageIndex = 0; pageIndex < 2; pageIndex++) {
			const uint32_t address = pageIndex == 0 ? SettingsAAddress : SettingsBAddress;
			for(uint32_t slot = 0; slot < RecordsPerPage; slot++) {
				Settings candidate; bool foreign = false;
				if(decodeSettings((const uint8_t *)(address + slot * RecordBytes), candidate, foreign)
					&& candidate.valid && (!best.valid || candidate.generation > best.generation)) {
					best = candidate; best.source = pageIndex == 0 ? Source::FlashA : Source::FlashB;
				}
			}
		}
		return best;
	}

	bool writeSettings(const Settings& requested) {
		if(requested.operatingCurrentMa < 50 || requested.operatingCurrentMa > 250) return false;
		if((requested.axisACalibrationValid && (requested.axisAThreshold < 16
			|| requested.axisAWidth < 8 || requested.axisAWidth > 4200))
			|| (requested.axisBCalibrationValid && (requested.axisBThreshold < 16
			|| requested.axisBWidth < 8 || requested.axisBWidth > 4200))) return false;
		const Settings before = readSettings();
		if(before.valid && before.operatingCurrentMa == requested.operatingCurrentMa
			&& before.fullCurrentHomeRecovery == requested.fullCurrentHomeRecovery
			&& before.opticalCalibrationVersion == requested.opticalCalibrationVersion
			&& before.axisACalibrationValid == requested.axisACalibrationValid
			&& before.axisAThreshold == requested.axisAThreshold
			&& before.axisAWidth == requested.axisAWidth
			&& before.axisBCalibrationValid == requested.axisBCalibrationValid
			&& before.axisBThreshold == requested.axisBThreshold
			&& before.axisBWidth == requested.axisBWidth) return true;
		uint32_t active = before.source == Source::FlashB ? SettingsBAddress : SettingsAAddress;
		uint32_t inactive = active == SettingsAAddress ? SettingsBAddress : SettingsAAddress;
		int slot = firstErased(active);
		uint32_t destination = active;
		bool compact = slot < 0;
		if(compact) { destination = inactive; slot = 0; }
		uint8_t record[RecordBytes];
		makeSettings(record, before.generation + 1U, requested);
		if(HAL_FLASH_Unlock() != HAL_OK) {
			log(LogLevel::Error, "PersistentStorage", "flash unlock failed");
			return false;
		}
		{
			char message[100];
			sprintf(message, "journal append: gen=%lu page=0x%08lX slot=%d compact=%d"
				, (unsigned long) (before.generation + 1U), (unsigned long) destination
				, slot, compact ? 1 : 0);
			log(LogLevel::Status, "PersistentStorage", message);
		}
		__HAL_FLASH_CLEAR_FLAG(FLASH_FLAG_OPTVERR);
		bool ok = false;
		if(compact) {
			ok = erasePage(destination) && programRecord(destination, record);
		} else {
			ok = programRecord(destination + (uint32_t) slot * RecordBytes, record);
			if(!ok) {
				// A probe-rs read/modify/write can program an all-ones doubleword while
				// preserving a page. It still reads as erased, but its hidden ECC bits make a
				// later append fail with PROGERR. The active page remains the committed copy,
				// so recover exactly like normal journal compaction: erase the alternate page,
				// write generation+1 there, and verify before selecting it.
				log(LogLevel::Warning, "PersistentStorage"
					, "append slot is ECC-programmed; compacting safely to alternate page");
				destination = inactive;
				slot = 0;
				ok = erasePage(destination) && programRecord(destination, record);
			}
		}
		HAL_FLASH_Lock();
		if(!ok) return false;
		const Settings after = readSettings();
		return after.valid && after.operatingCurrentMa == requested.operatingCurrentMa
			&& after.fullCurrentHomeRecovery == requested.fullCurrentHomeRecovery
			&& after.opticalCalibrationVersion == requested.opticalCalibrationVersion
			&& after.axisACalibrationValid == requested.axisACalibrationValid
			&& after.axisAThreshold == requested.axisAThreshold
			&& after.axisAWidth == requested.axisAWidth
			&& after.axisBCalibrationValid == requested.axisBCalibrationValid
			&& after.axisBThreshold == requested.axisBThreshold
			&& after.axisBWidth == requested.axisBWidth;
	}

	bool writeSettings(uint16_t currentMa, bool recovery) {
		Settings desired = readSettings();
		desired.operatingCurrentMa = currentMa;
		desired.fullCurrentHomeRecovery = recovery;
		return writeSettings(desired);
	}

	const char * sourceName(Source source) {
		switch(source) {
		case Source::FlashA: return "flash-a";
		case Source::FlashB: return "flash-b";
		default: return "defaults";
		}
	}
}
