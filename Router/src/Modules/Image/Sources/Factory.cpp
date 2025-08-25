#include "pch_App.h"
#include "Factory.h"

#include "FilePlayer.h"
#include "Gradient.h"
#include "Text.h"
#include "Spout.h"

namespace Modules {
	namespace Image {
		namespace Sources {
			//----------
			vector<Factory> factories;

			//----------
			void
				registerFactories()
			{
				registerFactory<FilePlayer>();
				registerFactory<Gradient>();
				registerFactory<Text>();
				registerFactory<Spout>();
			}

			//----------
			shared_ptr<Base>
				createFromJson(const nlohmann::json& json)
			{
				if (!json.contains("type")) {
					return nullptr;
				}
				auto type = (string)json["type"];

				// Look through factories
				for (const auto& factory : factories) {
					auto factoryNameSpace = ofSplitString(factory.typeName, "::", true, true);;
					if (factoryNameSpace.empty()) {
						continue;
					}

					auto strippedTypeName = factoryNameSpace.back();

					if (strippedTypeName == type) {
						return factory.createModule(json);
					}
				}

				// Didn't find a factory
				ofLogError() << "Couldn't find type : " << type;
				return nullptr;
			}
		}
	}
}