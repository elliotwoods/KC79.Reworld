#pragma once

#include "IDevice.h"
#include "ListedDevice.h"

namespace SerialDevices {
	class Serial : public IDevice
	{
	public:
		~Serial();
		string getTypeName() const override;
		bool open(const nlohmann::json&) override;
		bool open(string portAddress);
		void close() override;
		bool isConnected() override;

		size_t transmit(const Buffer&) override;

		bool hasDataIncoming() override;
		Buffer receiveBytes() override;

		static vector<ListedDevice> listDevices();
	protected:
		ofSerial serial;
	};
}