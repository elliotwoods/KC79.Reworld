#pragma once

#include "../Base.h"
#include "crow/crow.h"

namespace Modules {
	namespace REST {
		class Server : public Base
		{
		public:
			Server();
			~Server();

			string getTypeName() const override;
			void init() override;

			void start();
			void stop();

			void update() override;
			void populateInspector(ofxCvGui::InspectArguments& args);

			ofxCvGui::PanelPtr getMiniView();
		protected:
			struct : ofParameterGroup {
				ofParameter<bool> enabled{ "Enabled", true };
				ofParameter<int> port{ "Port", 8080 };
				PARAM_DECLARE("Server", enabled, port);
			} parameters;

			void setupCrowRoutes();

			shared_ptr<crow::SimpleApp> crow;
			std::future<void> crowRun;
		};
	}
}