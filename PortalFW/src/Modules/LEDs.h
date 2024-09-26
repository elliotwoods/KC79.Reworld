#pragma once

#include "Base.h"

#include <stdint.h>
#include <stddef.h>
#include <set>

#include "../Platform.h"
#include "MotionControl.h"

namespace Modules {
	class LEDs : public Base {
	public:
		const char * getTypeName() const override;
		void update() override;
		void setMotorIndicatorEnabled(bool);
	protected:
		bool motorIndicatorEnabled = true;
	};
}