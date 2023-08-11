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
		void uploadFirmwarePacket(uint16_t frameOffset
			, const char* packetData
			, size_t packetSize);
		void runApplication();

		void sendMagicWord(char, char);

		weak_ptr<RS485> rs485;

		struct : ofParameterGroup {
			struct : ofParameterGroup {
				ofParameter<bool> enabled{ "Announce", true };
				ofParameter<float> period{ "Period [s]", 0.2 };
				PARAM_DECLARE("Announce", enabled, period);
			} announce;

			struct : ofParameterGroup {
				ofParameter<int> frameSize{ "Frame size", FW_FRAME_SIZE };
				ofParameter<int> waitBetweenFrames{ "Wait between frames [ms]", 10 };
				PARAM_DECLARE("Upload", frameSize, waitBetweenFrames);
			} upload;

			PARAM_DECLARE("FWUpdate", announce, upload)
		} parameters;

		struct {
			chrono::system_clock::time_point lastSend{}; // initalise to 0
		} announce;
	};
}