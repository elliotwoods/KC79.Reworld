#pragma once

#include <stdint.h>

namespace PersistentStorage {
	static constexpr uint32_t IdentityAddress = 0x0801E800U;
	static constexpr uint32_t SettingsAAddress = 0x0801F000U;
	static constexpr uint32_t SettingsBAddress = 0x0801F800U;
	static constexpr uint32_t PageBytes = 2048U;
	static constexpr uint32_t RecordBytes = 64U;

	enum class Source : uint8_t { Defaults, FlashA, FlashB };

	struct Identity {
		bool valid = false;
		bool corrupt = false;
		bool foreignUID = false;
		uint32_t generation = 0;
		uint32_t serial = 0;
	};

	struct Settings {
		uint32_t generation = 0;
		// What a board with no valid settings record comes up at; App::setup applies it
		// unconditionally. Kept equal to MOTORDRIVERSETTINGS_DEFAULT_CURRENT -- a virgin board
		// that came up gentler than a provisioned one would be a trap, not a safety margin.
		uint16_t operatingCurrentMa = 250;
		bool fullCurrentHomeRecovery = true;
		uint16_t opticalCalibrationVersion = 0;
		bool axisACalibrationValid = false;
		uint8_t axisAThreshold = 0;
		int32_t axisAWidth = 0;
		bool axisBCalibrationValid = false;
		uint8_t axisBThreshold = 0;
		int32_t axisBWidth = 0;
		Source source = Source::Defaults;
		bool valid = false;
	};

	Identity readIdentity();
	Settings readSettings();
	bool writeSettings(const Settings&);
	bool writeSettings(uint16_t operatingCurrentMa, bool fullCurrentHomeRecovery);
	const char * sourceName(Source);
}
