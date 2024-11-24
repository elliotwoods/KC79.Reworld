#pragma once

#include "Base.h"
#include "Types.h"

namespace Modules {
	class KeyframeMotionControl : public Base{
	public:
		KeyframeMotionControl();
		const char * getTypeName() const;
		void update();
		void clear();

		bool processIncoming(Stream &) override;
	protected:
		struct Keyframe {
			Steps position = 0;
			StepsPerSecond velocity = 0;
		};

		Keyframe keyframes[2];

		uint32_t lastTimestamp = 0;
		uint32_t keyframeLifetime = 1000;
		bool active = false;
	};
}
