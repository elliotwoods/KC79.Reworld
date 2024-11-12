#pragma once

#include "Base.h"

namespace Modules {
	namespace Image {
		namespace Sources {
			class FilePlayer : public Base
			{
			public:
				MAKE_ENUM(LoopMode,
					(Loop, PingPong, None),
					("Loop", "Ping Pong", "None"));

				FilePlayer();
				string getTypeName() const override;

				void init() override;
				void update() override;
				void populateInspector(ofxCvGui::InspectArguments&);

				void render(const RenderSettings&) override;
			protected:
				void onPositionJumped(float&);
				bool disableJump = false;

				ofVideoPlayer player;
				ofPixels pixels8;

				struct : ofParameterGroup {
					ofParameter<std::filesystem::path> file{ "File", "" };
					ofParameter<bool> play{ "Play", true };
					ofParameter<LoopMode> loopMode{ "Loop Mode", LoopMode::Loop };
					ofParameter<float> speed{ "Speed", 1.0f };
					ofParameter<float> position{ "Position [s]", 0.0f, 0.0f, 1.0f};
					PARAM_DECLARE("FilePlayer", file, play, speed, position);
				} parameters;
			};
		}
	}
}