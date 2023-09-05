#pragma once

#include "IDevice.h"
#include "ListedDevice.h"
#include "ofxNetwork.h"

#define TCP_DEFAULT_PORT 4196
#define TCP_DEFAULT_TIMEOUT_S 1

namespace SerialDevices {
	class TCP : public IDevice
	{
	public:
		~TCP();
		string getTypeName() const override;
		bool open(const nlohmann::json&) override;
		bool open(string address, int port, int timeout_s);
		void close() override;
		bool isConnected() override;

		size_t transmit(const Buffer&) override;

		bool hasDataIncoming() override;
		Buffer receiveBytes() override;

		static vector<ListedDevice> listDevices();
	protected:
		ofxTCPClient tcpClient;
	};
}