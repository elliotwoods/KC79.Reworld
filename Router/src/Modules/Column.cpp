#include "pch_App.h"
#include "Column.h"

using namespace msgpack11;

namespace Modules {
	//----------
	Column::Column()
	{
		// RS485
		{
			this->rs485 = make_shared<RS485>(this);
			this->modules.push_back(this->rs485);
		}

		// FWUpdate
		{
			this->fwUpdate = make_shared<FWUpdate>(this->rs485);
			this->modules.push_back(this->fwUpdate);
		}

		// Init modules
		for (auto module : this->modules) {
			module->init();
		}

		this->onPopulateInspector += [this](ofxCvGui::InspectArguments& args) {
			this->populateInspector(args);
		};
	}

	//----------
	string
		Column::getTypeName() const
	{
		return "Column";
	}

	//----------
	string
		Column::getName() const
	{
		if (!this->name.empty()) {
			return this->name;
		}
		else {
			return this->getTypeName();
		}
	}
	
	//----------
	void
		Column::deserialise(const nlohmann::json& json)
	{
		// Build columns
		{
			if (json.contains("panelCount")) {
				auto panelCount = (int)json["panelCount"];
				this->buildPanels(panelCount);
			}
			else {
				this->buildPanels(1);
			}
		}

		// Deserialise all submodules with json 
		for (auto module : this->modules) {
			auto typeName = ofToLower(module->getTypeName());
			if (json.contains(typeName)) {
				module->deserialise(json[typeName]);
			}
		}

		// Set our name to be our index
		{
			if (json.contains("index")) {
				this->name = ofToString((int)json["index"]);
			}
		}

		// Physical properties
		{
			if (json.contains("physical")) {
				const auto& jsonPhysical = json["physical"];

				if (jsonPhysical.contains("flipped")) {
					this->parameters.physical.flipped = (bool)jsonPhysical["flipped"];
				}
			}
		}
	}

	//----------
	void
		Column::init()
	{

	}

	//----------
	void
		Column::update()
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

		if (this->parameters.scheduledPoll.enabled) {
			auto now = chrono::system_clock::now();
			auto deadline = this->lastPollAll + chrono::milliseconds((int)(this->parameters.scheduledPoll.period_s.get() * 1000.0f));
			if (now >= deadline) {
				this->pollAll();
			}
		}
	}

	//----------
	void
		Column::populateInspector(ofxCvGui::InspectArguments& args)
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
			if (!this->parameters.physical.flipped) {
				// Draw right way up
				for (const auto& it : this->portalsByID) {
					auto target = it.first;
					auto portal = it.second;
					auto rowIndex = (target - 1) / 3;

					// make a new row if we're on last
					if (widgetRows.find(rowIndex) == widgetRows.end()) {
						widgetRows.emplace(rowIndex, make_shared<ofxCvGui::Widgets::HorizontalStack>());
					}

					widgetRows[rowIndex]->add(Portal::makeButton(portal));
				}
			}
			else {
				// Draw upside-down
				int index = 0;
				for (auto it = this->portalsByID.rbegin(); it != this->portalsByID.rend(); it++) {
					auto target = it->first;
					auto portal = it->second;
					auto rowIndex = index / 3;

					// make a new row if we're on last
					if (widgetRows.find(rowIndex) == widgetRows.end()) {
						widgetRows.emplace(rowIndex, make_shared<ofxCvGui::Widgets::HorizontalStack>());
					}

					widgetRows[rowIndex]->add(Portal::makeButton(portal));

					index++;
				}
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

			inspector->addParameterGroup(this->parameters);

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
		Column::processIncoming(const nlohmann::json& json)
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
		Column::buildPanels(size_t panelCount)
	{
		this->portals.clear();
		this->portalsByID.clear();

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
	vector<shared_ptr<Portal>>
		Column::getAllPortals() const
	{
		return this->portals;
	}

	//----------
	shared_ptr<Portal>
		Column::getPortalByTargetID(Portal::Target targetID)
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
	shared_ptr<RS485>
		Column::getRS485()
	{
		return this->rs485;
	}

	//----------
	shared_ptr<FWUpdate>
		Column::getFWUpdate()
	{
		return this->fwUpdate;
	}

	//----------
	void
		Column::pollAll()
	{
		for (auto portal : this->portals) {
			portal->poll();
		}
		this->lastPollAll = chrono::system_clock::now();
	}

	//----------
	void
		Column::broadcast(const msgpack11::MsgPack& message)
	{
		auto packet = RS485::Packet(
			msgpack11::MsgPack::array{
				-1
				, (int8_t)0
				, message
			});
		packet.needsACK = false;
		this->rs485->transmit(packet);
	}

	//----------
	void
		Column::broadcastHome()
	{
		this->broadcast(msgpack11::MsgPack::object{
					{
						"home", msgpack11::MsgPack()
					}
			});
	}

	//----------
	ofxCvGui::ElementPtr
		Column::getMiniView(float width)
	{
		auto element = ofxCvGui::makeElement();
		const auto cellSize = width / 3.0f;
		element->setWidth(cellSize * 3.0f);
		auto portals = this->getAllPortals();
		element->setHeight(portals.size() / 3 * cellSize);
		element->onDraw += [this](ofxCvGui::DrawArguments& args) {
			const auto cellSize = args.localBounds.width / 3.0f;

			auto portals = this->getAllPortals();

			float y = cellSize * (portals.size() / 3);
			float x = 0.0f;
			int index = 0;
			const auto radius = cellSize / 2.0f - 3.0f;

			ofPushMatrix();
			{
				if (this->parameters.physical.flipped.get()) {
					auto height = cellSize * (portals.size() / 3);
					auto width = cellSize * 3;
					ofTranslate(width, height);
					ofRotateDeg(180.0f);
				}

				for (auto portal : portals) {
					auto pilot = portal->getPilot();
					const auto targetPosition = pilot->getPosition();
					const auto livePosition = pilot->getLivePosition();
					ofPushMatrix();
					{
						ofTranslate(x + cellSize / 2.0f, y + cellSize / 2.0f);
						ofPushStyle();
						{
							ofSetColor(100.0f);
							ofNoFill();
							ofDrawCircle(0, 0, radius);
							ofDrawLine(-radius, 0, radius, 0);
							ofDrawLine(0, -radius, 0, radius);

							ofSetColor(255.0f);
							ofFill();
							ofDrawCircle(radius * targetPosition.x, radius * targetPosition.y, 4.0f);

							ofSetColor(100, 100, 200);
							ofDrawCircle(radius * livePosition.x, radius * livePosition.y, 2.0f);
						}
						ofPopStyle();
					}
					ofPopMatrix();


					index++;

					if (index % 3 == 0) {
						y -= cellSize;
						x = 0.0f;
					}
					else {
						x += cellSize;
					}
				}
			}
			ofPopMatrix();
		};
		return element;
	}

	//----------
	void
		Column::refreshPortalsByID()
	{
		this->portalsByID.clear();
		for (auto portal : this->portals) {
			this->portalsByID.emplace(portal->getTarget(), portal);
		}
		this->portalsByIDDirty = false;
	}
}