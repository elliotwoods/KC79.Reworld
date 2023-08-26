#pragma once

#include "Base.h"
#include "RS485.h"

#define FW_FRAME_SIZE 32

namespace Modules {
	class FWUpdate : public Base
	{
	public:
		FWUpdate(shared_ptr<RS485>);

		string getTypeName() const override;
		void init() override;
		void update() override;

		void populateInspector(ofxCvGui::InspectArguments&);
		void uploadFirmware(const string& path);
	protected:
		void announceFirmware();
		void eraseFirmware();
		void uploadFirmwarePacket(uint32_t frameOffset
			, uint8_t* packetData
			, size_t packetSize);
		void runApplication();

		void sendMagicWord(char, char);

		weak_ptr<RS485> rs485;

		struct : ofParameterGroup {
			struct : ofParameterGroup {
				ofParameter<bool> enabled{ "Enabled", false };
				ofParameter<float> period{ "Period [s]", 0.2 };
				PARAM_DECLARE("Announce", enabled, period);
			} announce;

			struct : ofParameterGroup {
				ofParameter<bool> truncate{ "Truncate", false };
				ofParameter<int> frameSize{ "Frame size", FW_FRAME_SIZE };
				ofParameter<int> waitBetweenFrames{ "Wait between frames [ms]", 10 };
				ofParameter<int> frameRepetitions{ "Frame repetitions", 1 };
				PARAM_DECLARE("Upload", truncate, frameSize, waitBetweenFrames, frameRepetitions);
			} upload;

			PARAM_DECLARE("FWUpdate", announce, upload)
		} parameters;

		struct {
			chrono::system_clock::time_point lastSend{}; // initalise to 0
		} announce;
	};
}