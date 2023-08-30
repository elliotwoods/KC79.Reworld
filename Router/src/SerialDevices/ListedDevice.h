#pragma once

#include "IDevice.h"

namespace SerialDevices {
	struct ListedDevice
	{
		string type;
		string address;
		string name;
		function<shared_ptr<IDevice>()> createDevice;
	};
}