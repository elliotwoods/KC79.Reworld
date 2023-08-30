#include "pch_App.h"
#include "TCP.h"

namespace SerialDevices {
	//----------
	TCP::~TCP()
	{
		if (this->tcpClient.isConnected()) {
			this->tcpClient.close();
		}
	}

	//----------
	string
		TCP::getTypeName() const
	{
		return "TCP";
	}

	//----------
	bool
		TCP::open(const nlohmann::json& json)
	{
		if (json.contains("address") && json.contains("port")) {
			auto address = (string)json["address"];
			auto port = (int)json["port"];
			return this->open(address, port);
		}
		else {
			return false;
		}
	}

	//----------
	bool
		TCP::open(string address, int port)
	{
		return this->tcpClient.setup(address, port);
	}

	//----------
	void
		TCP::close()
	{
		this->tcpClient.close();
	}

	//----------
	size_t
		TCP::transmit(const Buffer& buffer)
	{
		if (this->tcpClient.sendRawBytes((char*)buffer.data()
			, buffer.size())) {
			return buffer.size();
		}
		else {
			return 0;
		}
	}

	//----------
	bool
		TCP::hasDataIncoming()
	{
		return this->tcpClient.getNumReceivedBytes() > 0;
	}

	//----------
	Buffer
		TCP::receiveBytes()
	{
		Buffer buffer;
		buffer.resize(256);

		auto bytesReceived = this->tcpClient.receiveRawBytes((char*)buffer.data(), buffer.size());
		if (bytesReceived < 0) {
			return Buffer();
		}
		else {
			buffer.resize(bytesReceived);
			return buffer;
		}
	}

	//----------
	vector<ListedDevice>
		TCP::listDevices()
	{
		vector<ListedDevice> listedDevices;

		auto typeName = TCP().getTypeName();

		// Default devices
		{
			int port = 4196;
			{
				string address = "192.168.1.201";

				ListedDevice listedDevice;
				{
					listedDevice.type = typeName;
					listedDevice.name = address;
					listedDevice.createDevice = [address, port]() {
						auto device = make_shared<TCP>();
						if (device->open(address, port)) {
							return static_pointer_cast<IDevice>(device);
						}
						return shared_ptr<IDevice>();
					};
				}
				listedDevices.push_back(listedDevice);
			}

			{
				string address = "192.168.1.202";

				ListedDevice listedDevice;
				{
					listedDevice.type = typeName;
					listedDevice.name = address;
					listedDevice.createDevice = [address, port]() {
						auto device = make_shared<TCP>();
						if (device->open(address, port)) {
							return static_pointer_cast<IDevice>(device);
						}
						return shared_ptr<IDevice>();
					};
				}
				listedDevices.push_back(listedDevice);
			}
		}
		// Custom device
		{
			ListedDevice listedDevice;
			{
				listedDevice.type = typeName;
				listedDevice.name = "Custom...";
				listedDevice.createDevice = []() {
					auto response = ofSystemTextBoxDialog("Hostname:Port");

					auto responseSplit = ofSplitString("response", ",", true, true);
					if (responseSplit.size() > 2) {
						auto address = responseSplit[0];
						auto port = ofToInt(responseSplit[1]);

						auto device = make_shared<TCP>();
						if (device->open(address, port)) {
							return static_pointer_cast<IDevice>(device);
						}
					}

					return shared_ptr<IDevice>();
				};
			}
		}
		return listedDevices;
	}
}