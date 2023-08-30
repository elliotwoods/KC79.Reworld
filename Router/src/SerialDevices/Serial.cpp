#include "pch_App.h"
#include "Serial.h"

namespace SerialDevices {
	//----------
	Serial::~Serial()
	{
		if (this->serial.isInitialized()) {
			this->serial.close();
		}
	}

	//----------
	string
		Serial::getTypeName() const
	{
		return "Serial";
	}

	//----------
	bool
		Serial::open(const nlohmann::json& json)
	{
		if (json.contains("portName")) {
			auto portName = (string)json["portName"];
			return this->open(portName);
		}
		else {
			return false;
		}
	}

	//----------
	bool
		Serial::open(string portAddress)
	{
		return this->serial.setup(portAddress, BAUD_RATE);
	}

	//----------
	void
		Serial::close()
	{
		this->serial.close();
	}

	//----------
	size_t
		Serial::transmit(const Buffer & buffer)
	{
		return (size_t) this->serial.writeBytes((char *) buffer.data()
			, buffer.size());
	}

	//----------
	bool
		Serial::hasDataIncoming()
	{
		return this->serial.available() > 0;
	}

	//----------
	Buffer
		Serial::receiveBytes()
	{
		Buffer buffer;

		auto bytesAvailable = this->serial.available();
		if (bytesAvailable > 0) {
			buffer.resize(bytesAvailable);
			serial.readBytes(buffer.data(), bytesAvailable);
			return buffer;
		}

		return buffer;
	}

	//----------
	vector<ListedDevice>
		Serial::listDevices()
	{
		vector<ListedDevice> listedDevices;

		auto typeName = Serial().getTypeName();

		ofSerial serial;
		auto devices = serial.getDeviceList();
		for (auto& device : devices) {
			auto devicePath = device.getDevicePath();
			ListedDevice listedDevice;
			{
				listedDevice.type = typeName;
				listedDevice.address = device.getDevicePath();
				listedDevice.name = device.getDeviceName();
				listedDevice.createDevice = [devicePath]() {
					auto serialDevice = make_shared<Serial>();
					if (serialDevice->open(devicePath)) {
						return static_pointer_cast<IDevice>(serialDevice);
					}
					else {
						return shared_ptr<IDevice>();
					}
				};
			}
			listedDevices.push_back(listedDevice);
		}

		// Custom device
		{
			ListedDevice listedDevice;
			{
				listedDevice.type = typeName;
				listedDevice.name = "Custom...";
				listedDevice.createDevice = []() {
					auto response = ofSystemTextBoxDialog("Port name");

					auto serialDevice = make_shared<Serial>();
					if (serialDevice->open(response)) {
						return static_pointer_cast<IDevice>(serialDevice);
					}
					else {
						return shared_ptr<IDevice>();
					}
				};
				listedDevices.push_back(listedDevice);
			}
		}
		return listedDevices;
	}
}