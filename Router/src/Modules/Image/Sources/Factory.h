#pragma once

#include "Base.h"

namespace Modules {
	namespace Image {
		namespace Sources {
			struct Factory
			{
				string typeName;
				function<shared_ptr<Base>(const nlohmann::json&)> createModule;
			};

			extern vector<Factory> factories;

			template<typename T>
			void registerFactory()
			{
				Factory factory;
				{
					factory.typeName = T().getTypeName();
					factory.createModule = [](const nlohmann::json& json) {
						auto module = make_shared<T>();
						module->deserialise(json);
						return module;
						};
				}
				factories.push_back(factory);
			}

			void registerFactories();

			shared_ptr<Base> createFromJson(const nlohmann::json&);
		}
	}
}