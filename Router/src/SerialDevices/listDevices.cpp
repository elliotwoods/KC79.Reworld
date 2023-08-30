#include "pch_App.h"
#include "listDevices.h"

#include "Serial.h"
#include "TCP.h"

namespace SerialDevices {
	vector<ListedDevice>
		listDevices()
	{
		vector<ListedDevice> listedDevices;

		// Serial
		{
			auto devices = Serial::listDevices();
			listedDevices.insert(listedDevices.end(), devices.begin(), devices.end());
		}

		// TCP
		{
			auto devices = TCP::listDevices();
			listedDevices.insert(listedDevices.end(), devices.begin(), devices.end());
		}

		return listedDevices;
	}
}