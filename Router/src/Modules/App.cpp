#include "pch_App.h"
#include "App.h"
#include "..\Utils.h"

using namespace msgpack11;

namespace Modules {
	//----------
	App::App()
	{
		{
			this->rs485 = make_shared<RS485>(this);
			this->modules.push_back(this->rs485);
		}

		{
			this->fwUpdate = make_shared<FWUpdate>(this->rs485);
			this->modules.push_back(this->fwUpdate);
		}

		this->buildPanels(1);

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
		for (auto module : this->modules) {
			module->init();
		}

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
	}

	//----------
	void
		App::update()
	{
		if (this->portalsByIDDirty) {
			this->refreshPortalsByID();
		}

		for (auto module : this->modules) {
			module->update();
		}

		for (auto portal : this->portals) {
			portal->update();
		}
	}

	//----------
	void
		App::populateInspector(ofxCvGui::InspectArguments& args)
	{
		auto inspector = args.inspector;

		inspector->addFps();

		// Add modules
		for (auto module : this->modules) {
			module->addSubMenuToInsecptor(inspector, module);
		}

		// Panel builder
		{
			inspector->addButton("Build panels", [this]() {
				auto response = ofSystemTextBoxDialog("Panel count");
				if (!response.empty()) {
					auto panelCount = ofToInt(response);
					if (panelCount > 0 && panelCount < 16) {
						this->buildPanels(panelCount);
					}
				}
				})->setDrawGlyph(u8"\uf0fe");
		}

		// Add portals
		{
			map<int, shared_ptr<ofxCvGui::Widgets::HorizontalStack>> widgetRows;
			for (const auto & it : this->portalsByID) {
				auto target = it.first;
				auto portal = it.second;
				auto rowIndex = (target - 1) / 3;
				
				if (widgetRows.find(rowIndex) == widgetRows.end()) {
					widgetRows.emplace(rowIndex, make_shared<ofxCvGui::Widgets::HorizontalStack>());
				}
				widgetRows[rowIndex]->add(Utils::makeButton(portal));
			}
			for (auto it = widgetRows.rbegin(); it != widgetRows.rend(); it++) {
				inspector->add(it->second);
			}
		}

		// Broadcast actions
		{
			inspector->addTitle("Broadcast:", ofxCvGui::Widgets::Title::Level::H3);

			auto buttonStack = inspector->addHorizontalStack();

			// Special action for poll
			buttonStack->addButton("Poll", [this]() {
				for(auto portal : this->portals) {
					portal->poll();
				}
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

			// Add simple pilot (draggable button
			{
				inspector->addTitle("Pilot all:", ofxCvGui::Widgets::Title::Level::H3);

				auto pilotButton = inspector->addButton("", []() {});
				pilotButton->setHeight(inspector->getWidth());

				// Add mouse action
				auto pilotButtonWeak = weak_ptr<ofxCvGui::Element>(pilotButton);
				pilotButton->onMouse += [this, pilotButtonWeak](ofxCvGui::MouseArguments& args) {
					if (this->portals.empty()) {
						return;
					}

					auto pilotButton = pilotButtonWeak.lock();
					if (pilotButton->isMouseDown()) {
						auto firstPortal = this->portals.front();
						auto firstPilot = firstPortal->getPilot();

						auto position = args.localNormalized * 2.0f - 1.0f;
						position.y *= -1.0f; // position +y is up

						// clamp to r<=1
						if (glm::length(position) > 1.0f) {
							position /= glm::length(position);
						}

						auto polar = firstPilot->positionToPolar(position);
						auto axes = firstPilot->polarToAxes(position);

						auto stepsA = firstPilot->axisToSteps(axes[0], 0);
						auto stepsB = firstPilot->axisToSteps(axes[1], 1);

						this->broadcast(MsgPack::object{
							{
								"m"
								, MsgPack::array {
									stepsA
									, stepsB
								}
							}
							});
					}
				};
			}
		}
		
	}

	//----------
	void
		App::processIncoming(const nlohmann::json& json)
	{
		if (json.size() >= 3) {
			// It's a packet
			auto target = (Portal::Target)json[0];
			auto origin = (Portal::Target)json[1];
			auto message = json[2];

			// Route message to portal
			if (target == 0) {
				for (auto& it : this->portalsByID) {
					if (it.first == origin) {
						it.second->processIncoming(message);
					}
				}
			}
		}
	}

	//----------
	void
		App::dragEvent(const ofDragInfo& dragInfo)
	{
		for (const auto& file : dragInfo.files) {
			this->fwUpdate->uploadFirmware(file);
		}
	}

	//----------
	void
		App::buildPanels(size_t panelCount)
	{
		auto rows = panelCount * 3;
		for (int j = 0; j < rows; j++) {
			for (int i = 0; i < 3; i++) {
				auto target = i + j * 3 + 1;
				auto portal = make_shared<Portal>(this->rs485, target);
				portal->onTargetChange += [this](Portal::Target) {
					this->portalsByIDDirty = true;
				};
				this->portals.push_back(portal);
			}
		}
		this->portalsByIDDirty = true;
		ofxCvGui::refreshInspector(this);
	}

	//----------
	shared_ptr<Portal>
		App::getPortalByTargetID(Portal::Target targetID)
	{
		auto findPortal = this->portalsByID.find(targetID);
		if (findPortal == this->portalsByID.end()) {
			// Empty response = not found
			return shared_ptr<Portal>();
		}
		else {
			return findPortal->second;
		}
	}

	//----------
	void
		App::broadcast(const msgpack11::MsgPack& message)
	{
		this->rs485->transmit(msgpack11::MsgPack::array{
			-1
			, (int8_t)0
			, message
		}, false);
	}

	//----------
	void
		App::refreshPortalsByID()
	{
		this->portalsByID.clear();
		for (auto portal : this->portals) {
			this->portalsByID.emplace(portal->getTarget(), portal);
		}
		this->portalsByIDDirty = false;
	}

	//----------
	void
		App::setupCrowRoutes()
	{
		CROW_ROUTE(crow, "/")([this]() {
			// test response
			return crow::response(200, "true");
			});

		CROW_ROUTE(crow, "/<int>/<int>/setPosition/<float>,<float>")([this](int col, int module, float x, float y) {
			// for now we ignore the column

			// Get the portal
			auto portal = this->getPortalByTargetID(module);
			if (!portal) {
				return crow::response(500, "Portal not found");
			}

			if (glm::length(glm::vec2{ x, y }) > 1.0f) {
				return crow::response(500, "Out of range");
			}

			portal->getPilot()->setPosition({ x, y });

			return crow::response(200, "true");
			});

		CROW_ROUTE(crow, "/<int>/<int>/getPosition")([this](int col, int module) {
			// for now we ignore the column

			// Get the portal
			auto portal = this->getPortalByTargetID(module);
			if (!portal) {
				return crow::response(500, "Portal not found");
			}

			crow::json::wvalue json;
			auto position = portal->getPilot()->getLivePosition();
			json["x"] = position.x;
			json["y"] = position.y;

			return crow::response(200, json);
			});

		CROW_ROUTE(crow, "/<int>/<int>/getTargetPosition")([this](int col, int module) {
			// for now we ignore the column

			// Get the portal
			auto portal = this->getPortalByTargetID(module);
			if (!portal) {
				return crow::response(500, "Portal not found");
			}

			crow::json::wvalue json;
			auto position = portal->getPilot()->getLiveTargetPosition();
			json["x"] = position.x;
			json["y"] = position.y;

			return crow::response(200, json);
			});

		CROW_ROUTE(crow, "/<int>/<int>/isInPosition")([this](int col, int module) {
			// for now we ignore the column

			// Get the portal
			auto portal = this->getPortalByTargetID(module);
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

		CROW_ROUTE(crow, "/<int>/<int>/pollPosition")([this](int col, int module) {
			// for now we ignore the column

			// Get the portal
			auto portal = this->getPortalByTargetID(module);
			if (!portal) {
				return crow::response(500, "Portal not found");
			}

			portal->getPilot()->pollPosition();

			return crow::response(200, "true");
			});

		CROW_ROUTE(crow, "/<int>/<int>/push")([this](int col, int module) {
			// for now we ignore the column

			// Get the portal
			auto portal = this->getPortalByTargetID(module);
			if (!portal) {
				return crow::response(500, "Portal not found");
			}

			portal->getPilot()->push();

			return crow::response(200, "true");
			});
	}
}