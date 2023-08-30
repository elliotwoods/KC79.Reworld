#include "pch_App.h"
#include "Factory.h"

#include "Serial.h"
#include "TCP.h"

namespace SerialDevices {
	//----------
	vector<Factory> factories;

	//----------
	void
		registerFactories()
	{
		registerFactory<Serial>();
		registerFactory<TCP>();
	}

	//----------
	shared_ptr<IDevice>
		createFromJson(const nlohmann::json& json)
	{
		if (!json.contains("deviceType")) {
			return nullptr;
		}
		auto deviceType = (string) json["deviceType"];

		// Look through factories
		for (const auto& factory : factories) {
			if (factory.typeName == deviceType) {
				return factory.createDevice(json);
			}
		}

		// Didn't find a factory
		ofLogError() << "Couldn't find device type : " << deviceType;
		return nullptr;
	}
}