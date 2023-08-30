#pragma once

#include "IDevice.h"
#include "ListedDevice.h"
#include "ofxNetwork.h"

namespace SerialDevices {
	class TCP : public IDevice
	{
	public:
		~TCP();
		string getTypeName() const override;
		bool open(const nlohmann::json&) override;
		bool open(string address, int port);
		void close() override;

		size_t transmit(const Buffer&) override;

		bool hasDataIncoming() override;
		Buffer receiveBytes() override;

		static vector<ListedDevice> listDevices();
	protected:
		ofxTCPClient tcpClient;
	};
}