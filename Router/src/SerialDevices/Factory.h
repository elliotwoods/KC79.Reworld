#pragma once

#include "IDevice.h"
#include "TCP.h"

namespace SerialDevices {
	struct Factory
	{
		string typeName;
		function<shared_ptr<IDevice>(const nlohmann::json&)> createDevice;
	};

	extern vector<Factory> factories;
	
	template<typename T>
	void registerFactory()
	{
		Factory factory;
		{
			factory.typeName = T().getTypeName();
			factory.createDevice = [](const nlohmann::json& json) {
				auto device = make_shared<T>();
				if (!device->open(json)) {
					ofLogError() << "Couldn't open serial device : " << json.dump(4);
					device.reset();
				}
				return device;
			};
		}
		factories.push_back(factory);
	}

	void registerFactories();

	shared_ptr<IDevice> createFromJson(const nlohmann::json&);
}