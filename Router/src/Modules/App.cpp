#include "pch_App.h"
#include "App.h"
#include "../Utils.h"

using namespace msgpack11;

namespace Modules {
	//----------
	App::App()
	{
		{
			this->setupCrowRoutes();
			this->crowRun = this->crow.port(8080).multithreaded().run_async();
			this->crow.loglevel(crow::LogLevel::Warning);
		}
	}

	//----------
	App::~App()
	{
		this->crow.stop();
		this->crowRun.get();
	}

	//----------
	string
		App::getTypeName() const
	{
		return "App"; 
	}

	//----------
	void
		App::init()
	{
		this->onPopulateInspector += [this](ofxCvGui::InspectArguments& args) {
			this->populateInspector(args);
		};

		ofxCvGui::InspectController::X().onClear += [this](ofxCvGui::InspectArguments& args) {
			// Add the title of selected submodule to top of inspector
			auto inspected = ofxCvGui::InspectController::X().getTarget();
			if (inspected) {
				auto submodule = dynamic_pointer_cast<Modules::Base>(inspected);
				if (submodule) {
					auto inspector = args.inspector;
					inspector->addTitle(submodule->getName());
				}
			}
		};

		// Load the config.json file
		{
			auto file = ofFile("config.json");
			if (file.exists()) {
				auto buffer = file.readToBuffer();
				auto json = nlohmann::json::parse(buffer);

				if (json.contains("columns")) {
					const auto& jsonColumns = json["columns"];
					for (const auto& jsonColumn : jsonColumns) {
						auto column = make_shared<Column>(jsonColumn);
						if (jsonColumn.contains("index")) {
							this->columns.emplace((int)jsonColumn["index"], column);
							column->init();
						}
						else {
							ofLogError("config.json") << "Config file needs to set an index for each column";
						}
					}
				}
			}
			else {
				// create a blank column
				auto column = make_shared<Column>();
				column->init();
				this->columns.emplace(1, column);
				column->init();
			}
		}
	}

	//----------
	void
		App::update()
	{
		for (const auto& column : this->columns) {
			column.second->update();
		}
	}

	//----------
	void
		App::populateInspector(ofxCvGui::InspectArguments& args)
	{
		auto inspector = args.inspector;

		inspector->addFps();

		// Add columns
		for (const auto & column : this->columns) {
			column.second->addSubMenuToInsecptor(inspector, column.second);
		}
	}

	//----------
	shared_ptr<Column>
		App::getColumnByID(int id) const
	{
		if (this->columns.find(id) == this->columns.end()) {
			return shared_ptr<Column>();
		}
		else {
			return this->columns.at(id);
		}
	}

	//----------
	void
		App::dragEvent(const ofDragInfo& dragInfo)
	{

	}


	//----------
	void
		App::setupCrowRoutes()
	{
		CROW_ROUTE(crow, "/")([this]() {
			// test response
			return crow::response(200, "true");
			});

		CROW_ROUTE(crow, "/<int>/<int>/setPosition/<float>,<float>")([this](int col, int portal_id, float x, float y) {
			// Get the column
			auto column = this->getColumnByID(col);
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
			// Get the column
			auto column = this->getColumnByID(col);
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
			// Get the column
			auto column = this->getColumnByID(col);
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
			// Get the column
			auto column = this->getColumnByID(col);
			if (!column) {
				return crow::response(500, "Column not found");
			}

			// Get the portal
			auto portal = column->getPortalByTargetID(portal_id);
			if (!portal) {
				return crow::response(500, "Portal not found");
			}

			if (portal->getPilot()->isInTargetPosition()) {
				return crow::response(200, crow::json::wvalue( true ));
			}
			else {
				return crow::response(200, crow::json::wvalue( false ));
			}
			});

		CROW_ROUTE(crow, "/<int>/<int>/pollPosition")([this](int col, int portal_id) {
			// Get the column
			auto column = this->getColumnByID(col);
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
			// Get the column
			auto column = this->getColumnByID(col);
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