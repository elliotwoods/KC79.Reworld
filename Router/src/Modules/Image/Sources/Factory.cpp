#include "pch_App.h"
#include "Factory.h"

#include "FilePlayer.h"
#include "Gradient.h"
#include "Text.h"

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
					if (factory.typeName == type) {
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