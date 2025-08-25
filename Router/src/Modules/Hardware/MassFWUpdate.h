#pragma once

#include "../Base.h"
#include "RS485.h"

#define FW_FRAME_SIZE 32

namespace Modules {
	namespace Hardware {
		class MassFWUpdate : public Base
		{
		public:
			MassFWUpdate();

			string getTypeName() const override;
			void init() override;
			void update() override;

			void populateInspector(ofxCvGui::InspectArguments&);
			void uploadFirmware(const string& path, const function<void(const string&)>& onProgress = nullptr);
		protected:
			vector<shared_ptr<RS485>> getRS485s() const;

			void announceFirmware(shared_ptr<RS485>);
			void eraseFirmware(shared_ptr<RS485>);
			void uploadFirmwarePacket(shared_ptr<RS485>
				, uint32_t frameOffset
				, uint8_t* packetData
				, size_t packetSize);
			void runApplication(shared_ptr<RS485>);

			void sendMagicWord(shared_ptr<RS485>, char, char);

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
					ofParameter<int> frameRepetitions{ "Frame repetitions", 6 };
					PARAM_DECLARE("Upload", truncate, frameSize, waitBetweenFrames, frameRepetitions);
				} upload;

				PARAM_DECLARE("MassFWUpdate", announce, upload)
			} parameters;

			struct {
				chrono::system_clock::time_point lastSend{}; // initalise to 0
			} announce;
		};
	}
}