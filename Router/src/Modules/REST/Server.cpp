#include "pch_App.h"
#include "Server.h"
#include "../App.h"

namespace Modules {
	namespace REST {
		//----------
		Server::Server()
		{
		}

		//----------
		Server::~Server()
		{
			this->stop();
		}

		//----------
		string
			Server::getTypeName() const
		{
			return "REST::Server";
		}

		//----------
		void
			Server::init()
		{
			{
				this->setupCrowRoutes();
			}
		}

		//----------
		void
			Server::start()
		{
			this->crow = make_shared<crow::SimpleApp>();

			this->setupCrowRoutes();
			this->crowRun = this->crow->port(this->parameters.port.get()).multithreaded().run_async();
			this->crow->loglevel(crow::LogLevel::Warning);

		}

		//----------
		void
			Server::stop()
		{
			if (!this->crow) {
				return;
			}

			this->crow->stop();
			this->crowRun.get();
			this->crow.reset();
		}

		//----------
		void
			Server::update()
		{
			// Check if should close 
			if (this->crow && !this->parameters.enabled) {
				this->stop();
			}

			// Check settings
			if (this->crow) {
				if (this->crow->port() != this->parameters.port) {
					this->stop();
				}
			}

			// Open the device
			if (!this->crow && this->parameters.enabled) {
				this->start();
			}
		}

		//----------
		void
			Server::populateInspector(ofxCvGui::InspectArguments& args)
		{
			auto inspector = args.inspector;
			inspector->addParameterGroup(this->parameters);
		}

		//----------
		ofxCvGui::PanelPtr
			Server::getMiniView()
		{
			auto view = make_shared<ofxCvGui::Panels::Base>();
			view->onDraw += [this](ofxCvGui::DrawArguments& args) {
				ofxCvGui::Utils::drawText(this->getName(), args.localBounds);
				};
			return view;
		}

		//----------
		void
			Server::setupCrowRoutes()
		{
			auto & crow = *this->crow;
			CROW_ROUTE(crow, "/")([this]() {
				// test response
				return crow::response(200, "true");
				});

			CROW_ROUTE(crow, "/<int>/<int>/setPosition/<float>,<float>")([this](int col, int portal_id, float x, float y) {
				auto app = App::X();
				auto installation = app->getInstallation();

				// Get the column
				auto column = installation->getColumnByID(col);
				if (!column) {
					return crow::response(500, "Column not found");
				}

				// Get the portal
				auto portal = column->getPortalByTargetID(portal_id);
				if (!portal) {
					return crow::response(500, "Portal not found");
				}

				if (glm::length(glm::vec2{ x, y }) > 1.0f) {
					return crow::response(500, "Out of range");
				}

				portal->getPilot()->setPosition({ x, y });

				return crow::response(200, "true");
				});

			CROW_ROUTE(crow, "/<int>/<int>/getPosition")([this](int col, int portal_id) {
				auto app = App::X();
				auto installation = app->getInstallation();

				// Get the column
				auto column = installation->getColumnByID(col);
				if (!column) {
					return crow::response(500, "Column not found");
				}

				// Get the portal
				auto portal = column->getPortalByTargetID(portal_id);
				if (!portal) {
					return crow::response(500, "Portal not found");
				}

				crow::json::wvalue json;
				auto position = portal->getPilot()->getLivePosition();
				json["x"] = position.x;
				json["y"] = position.y;

				return crow::response(200, json);
				});

			CROW_ROUTE(crow, "/<int>/<int>/getTargetPosition")([this](int col, int portal_id) {
				auto app = App::X();
				auto installation = app->getInstallation();

				// Get the column
				auto column = installation->getColumnByID(col);
				if (!column) {
					return crow::response(500, "Column not found");
				}

				// Get the portal
				auto portal = column->getPortalByTargetID(portal_id);
				if (!portal) {
					return crow::response(500, "Portal not found");
				}

				crow::json::wvalue json;
				auto position = portal->getPilot()->getLiveTargetPosition();
				json["x"] = position.x;
				json["y"] = position.y;

				return crow::response(200, json);
				});

			CROW_ROUTE(crow, "/<int>/<int>/isInPosition")([this](int col, int portal_id) {
				auto app = App::X();
				auto installation = app->getInstallation();

				// Get the column
				auto column = installation->getColumnByID(col);
				if (!column) {
					return crow::response(500, "Column not found");
				}

				// Get the portal
				auto portal = column->getPortalByTargetID(portal_id);
				if (!portal) {
					return crow::response(500, "Portal not found");
				}

				if (portal->getPilot()->isInTargetPosition()) {
					return crow::response(200, crow::json::wvalue(true));
				}
				else {
					return crow::response(200, crow::json::wvalue(false));
				}
				});

			CROW_ROUTE(crow, "/<int>/<int>/pollPosition")([this](int col, int portal_id) {
				auto app = App::X();
				auto installation = app->getInstallation();

				// Get the column
				auto column = installation->getColumnByID(col);
				if (!column) {
					return crow::response(500, "Column not found");
				}

				// Get the portal
				auto portal = column->getPortalByTargetID(portal_id);
				if (!portal) {
					return crow::response(500, "Portal not found");
				}

				portal->getPilot()->pollPosition();

				return crow::response(200, "true");
				});

			CROW_ROUTE(crow, "/<int>/<int>/push")([this](int col, int portal_id) {
				auto app = App::X();
				auto installation = app->getInstallation();

				// Get the column
				auto column = installation->getColumnByID(col);
				if (!column) {
					return crow::response(500, "Column not found");
				}

				// Get the portal
				auto portal = column->getPortalByTargetID(portal_id);
				if (!portal) {
					return crow::response(500, "Portal not found");
				}

				portal->getPilot()->push();

				return crow::response(200, "true");
				});
		}
	}
}