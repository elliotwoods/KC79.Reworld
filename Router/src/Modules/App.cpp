#include "pch_App.h"
#include "App.h"
#include "../Utils.h"
#include "../OSC/Routes.h"

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

		{
			this->testPattern = make_shared<TestPattern>(this);
			this->modules.push_back(this->testPattern);
		}

		this->tcpServer.setup(4444);
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
		this->load();

		// Initialise modules
		for(const auto& module : this->modules) {
			module->init();
		}
	}

	//----------
	void
		App::update()
	{
		// update modules
		for (const auto& module : this->modules) {
			module->update();
		}

		for (const auto& column : this->columns) {
			column.second->update();
		}

		// OSC
		{
			// Check if should close 
			if (this->oscReceiver && !this->parameters.osc.enabled) {
				this->oscReceiver.reset();
			}

			// Check settings
			if (this->oscReceiver) {
				if (this->oscReceiver->getPort() != this->parameters.osc.port) {
					this->oscReceiver.reset();
				}
			}

			// Open the device
			if (!this->oscReceiver && this->parameters.osc.enabled) {
				this->oscReceiver = make_shared<ofxOscReceiver>();
				this->oscReceiver->setup(this->parameters.osc.port);
				if (!this->oscReceiver->isListening()) {
					this->oscReceiver.reset();
				}
			}

			// Perform updates
			if (this->oscReceiver) {
				ofxOscMessage message;
				while (this->oscReceiver->getNextMessage(&message)) {
					OSC::handleRoute(message);
				}
			}
		}

		// Flash screen if no Rx on all columns
		{
			// store the background color if first frame
			if (ofGetFrameNum() < 10) {
				this->flashScreenSettings.normal = ofGetBackgroundColor();
			}
			else {
				// Check if has Rx on All columns
				bool hasRxOnAll = true;
				for (const auto& column : this->columns) {
					if (!column.second->getRS485()->hasRxBeenReceived()) {
						hasRxOnAll = false;
						break;
					}
				}

				if (!hasRxOnAll) {
					auto second = (int)ofGetElapsedTimef();
					ofSetBackgroundColor(
						second % 2 == 0
						? this->flashScreenSettings.normal
						: this->flashScreenSettings.flash
					);
				}
				else {
					ofSetBackgroundColor(this->flashScreenSettings.normal);
				}
			}
		}
	}

	//----------
	void
		App::load()
	{
		nlohmann::json json;

		// Load the file
		{
			auto file = ofFile("config.json");
			if (file.exists()) {
				auto buffer = file.readToBuffer();
				json = nlohmann::json::parse(buffer);
			}
		}

		// Deserialise
		this->deserialise(json);
	}

	//----------
	void
		App::deserialise(const nlohmann::json& json)
	{
		this->columns.clear();
		
		if (json.contains("columns")) {
			// Build up the columns
			const auto& jsonColumns = json["columns"];
			for (const auto& jsonColumn : jsonColumns) {
				auto column = make_shared<Column>();
				if (jsonColumn.contains("index")) {
					this->columns.emplace((int)jsonColumn["index"], column);
					column->deserialise(jsonColumn);
					column->init();
				}
				else {
					ofLogError("deserialise") << "Config json needs to set an index for each column";
				}
			}

			// Deserialise modules
			for (auto module : this->modules) {
				if (json.contains(module->getName())) {
					module->deserialise(json[module->getName()]);
				}
			}
		}
		else {
			// create a blank column
			auto column = make_shared<Column>();
			this->columns.emplace(1, column);
			column->init();
		}

		ofxCvGui::refreshInspector(this);
	}

	//----------
	void
		App::populateInspector(ofxCvGui::InspectArguments& args)
	{
		auto inspector = args.inspector;

		inspector->addFps();

		inspector->addSpacer();

		// Actions

		{
			auto buttonStack = inspector->addHorizontalStack();
			// Special action for poll
			buttonStack->addButton("Poll", [this]() {
				this->pollAll();
				}, ' ')->setDrawGlyph(u8"\uf059");

			// Add actions
			auto actions = Portal::getActions();
			for (const auto& action : actions) {
				auto hasHotkey = action.shortcutKey != 0;

				auto buttonAction = [this, action]() {
					this->broadcast(action.message);
				};

				auto button = hasHotkey
					? buttonStack->addButton(action.caption, buttonAction, action.shortcutKey)
					: buttonStack->addButton(action.caption, buttonAction);

				button->setDrawGlyph(action.icon);
			}
		}

		inspector->addButton("FW update...", [this]() {
			auto result = ofSystemLoadDialog("Load application.bin");
			if (result.bSuccess) {
				this->uploadFWAll(result.filePath);
			}
			});

		inspector->addSpacer();

		// Add columns
		{
			auto stack = inspector->addHorizontalStack();
			float colHeight = 10.0f;
			for (const auto& it : this->columns) {
				const auto& column = it.second;
				auto columnWeak = weak_ptr<Column>(column);
				auto button = make_shared<ofxCvGui::Widgets::SubMenuInspectable>("", column);
				
				// Custom draw
				{
					button->onDraw += [columnWeak](ofxCvGui::DrawArguments& args) {
					};
				}

				// Vertical stack of elements in the button
				float height = 0.0f;
				{

					auto verticalStack = make_shared<ofxCvGui::Widgets::VerticalStack>(ofxCvGui::Widgets::VerticalStack::Layout::UseElementHeight);
					{
						button->addChild(verticalStack);
						auto verticalStackWeak = weak_ptr<ofxCvGui::Element>(verticalStack);
						button->onBoundsChange += [verticalStackWeak](ofxCvGui::BoundsChangeArguments& args) {
							auto verticalStack = verticalStackWeak.lock();
							auto bounds = args.localBounds;
							bounds.width -= 60.0f;
							verticalStack->setBounds(bounds);
						};
					}

					// Title
					{
						auto element = make_shared<ofxCvGui::Widgets::Title>(column->getName());
						verticalStack->add(element);
						height += element->getHeight();
					}

					// RS485 connected
					{
						auto element = make_shared<ofxCvGui::Widgets::Indicator>("RS485", [columnWeak]() {
							auto column = columnWeak.lock();
							if (column->getRS485()->isConnected()) {
								return ofxCvGui::Widgets::Indicator::Status::Good;
							}
							else {
								return ofxCvGui::Widgets::Indicator::Status::Warning;
							}
							});
						verticalStack->add(element);
						height += element->getHeight();
					}

					// Outbox size
					{
						auto element = make_shared<ofxCvGui::Widgets::LiveValue<size_t>>("Outbox size", [columnWeak]() {
							auto column = columnWeak.lock();
							return column->getRS485()->getOutboxCount();
							});
						verticalStack->add(element);
						height += element->getHeight();
					}

					// Tx and Rx heartbeats
					{
						auto widgets = column->getRS485()->getWidgets();
						for (auto widget : widgets) {
							verticalStack->add(widget);
							height += widget->getHeight();
						}
					}

					// Spacer at bottom
					{
						auto element = make_shared<ofxCvGui::Widgets::Spacer>();
						verticalStack->add(element);
						height += element->getHeight();
					}
				}
				button->setHeight(height);
				colHeight = height;

				stack->add(button);
			}

			stack->setHeight(colHeight);
		}
		
		inspector->addIndicatorBool("OSC receiver open", [this]() {
			return (bool)this->oscReceiver;
			});


		// Add modules
		for (const auto& module : this->modules) {
			module->addSubMenuToInsecptor(inspector, module);
		}

		inspector->addSpacer();

		inspector->addParameterGroup(this->parameters);

	}

	//----------
	vector<shared_ptr<Column>>
		App::getAllColumns() const
	{
		vector<shared_ptr<Column>> columns;
		for (const auto& it : this->columns) {
			columns.push_back(it.second);
		}

		return columns;
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
	glm::tvec2<size_t>
		App::getSize() const
	{
		auto allColumns = this->getAllColumns();
		auto testColumn = allColumns.front();
		auto testPortals = testColumn->getAllPortals();

		return {
			this->columns.size() * 3
			, testPortals.size() / 3
		};
	}


	//----------
	void
		App::moveGrid(const vector<glm::vec2>& positions)
	{
		auto allColumns = this->getAllColumns();

		auto size = this->getSize();
		auto& width = size[0];
		auto& height = size[1];

		auto count = min(width * height, positions.size());
		for (int i = 0; i < count; i++) {
			auto x = i % width;
			auto y = i / width;

			auto column = allColumns[x / 3];
			auto portal_index = x % 3 + y * 3 + 1;

			auto portal = column->getPortalByTargetID(portal_index);
			if (!portal) {
				ofLogError("App::moveGrid") << "Couldn't get portal" << portal_index;
				continue;
			}

			const auto& position = positions[i];
			portal->getPilot()->setPosition(position);
		}
	}

	//----------
	void
		App::moveGridRow(const vector<glm::vec2>& positions, int rowIndex)
	{
		if (this->columns.empty()) {
			ofLogError("App::moveGrid") << "No column to test with";
		}
		auto allColumns = this->getAllColumns();
		auto testColumn = allColumns.front();
		auto testPortals = testColumn->getAllPortals();

		auto width = this->columns.size() * 3;
		auto height = testPortals.size() / 3;

		auto count = min(width, positions.size());

		for (int i = 0; i < count; i++) {
			auto x = i;
			auto y = rowIndex;

			auto column = allColumns[x / 3];
			auto portal_index = x % 3 + y * 3 + 1;

			auto portal = column->getPortalByTargetID(portal_index);
			if (!portal) {
				ofLogError("App::moveGrid") << "Couldn't get portal" << portal_index;
				continue;
			}

			portal->getPilot()->setPosition(positions[i]);
		}
	}

	//----------
	void
		App::pollAll()
	{
		for (const auto& it : this->columns) {
			auto column = it.second;
			column->pollAll();
		}
	}

	//----------
	void
		App::broadcast(const msgpack11::MsgPack& message)
	{
		for (const auto& it : this->columns) {
			auto column = it.second;
			column->broadcast(message);
		}
	}

	//----------
	void
		App::uploadFWAll(const string& path)
	{
		vector<std::future<void>> columnWaits;

		auto callback = [](const string& message) {
			cout << message;
		};

		for (const auto& it : this->columns) {
			auto column = it.second;
			columnWaits.push_back(async([column, path, &callback](){
				column->getFWUpdate()->uploadFirmware(path, callback);
				}
			));
		}

		for (auto& columnWait : columnWaits) {
			columnWait.wait();
		}
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