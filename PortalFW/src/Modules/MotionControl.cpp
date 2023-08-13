#include "MotionControl.h"

namespace Modules {
	MotionControl::MotionControl(MotorDriver& motorDriver, HomeSwitch& homeSwitch)
	: motorDriver(motorDriver)
	, homeSwitch(homeSwitch)
	{

	}
}