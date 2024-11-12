#include "pch_App.h"
#include "Column.h"

using namespace msgpack11;

namespace Modules {
	//----------
	Column::Column(const Settings& settings)
	{
		this->onPopulateInspector += [this](ofxCvGui::InspectArguments& args) {
			this->populateInspector(args);
			};

		// Submodules
		{

			this->rs485 = make_shared<RS485>(this);
			this->fwUpdate = make_shared<FWUpdate>(this->rs485);

			this->submodules = {
				this->rs485
				, this->fwUpdate
			};

			for (auto module : this->submodules) {
				module->init();
			}
		}

		this->columnIndex = settings.index;

		// Build portals
		{
			this->parameters.arrangement.countX = settings.countX;
			this->parameters.arrangement.countY = settings.countY;
			this->parameters.arrangement.flipped = settings.flipped;
			this->rebuildPortals();
		}
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
		return ofToString(this->columnIndex);
	}

	//----------
	void
		Column::deserialise(const nlohmann::json& json)
	{
		// Build portals
		{
			Utils::deserialize(json, this->parameters.arrangement.countX);
			Utils::deserialize(json, this->parameters.arrangement.countY);
			Utils::deserialize(json, this->parameters.arrangement.flipped);
			this->rebuildPortals();
		}

		// Deserialise all submodules with json 
		for (auto module : this->submodules) {
			auto typeName = ofToLower(module->getName());
			if (json.contains(typeName)) {
				module->deserialise(json[typeName]);
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

		for (auto module : this->submodules) {
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
		for (auto module : this->submodules) {
			module->addSubMenuToInsecptor(inspector, module);
		}



		// Portals
		{
			{
				inspector->addButton("Rebuild portals", [this]() {
					this->rebuildPortals();
					})->setDrawGlyph(u8"\uf0fe");
			}

			map<int, shared_ptr<ofxCvGui::Widgets::HorizontalStack>> widgetRows;
			if (!this->parameters.arrangement.flipped) {
				const auto countX = this->parameters.arrangement.countX.get();

				// Draw right way up
				for (const auto& it : this->portalsByID) {
					auto target = it.first;
					auto portal = it.second;
					auto rowIndex = (target - 1) / countX;

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
					auto rowIndex = index / countX;

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
				if (buttonStack->getElements().size() >= 6) {
					buttonStack = inspector->addHorizontalStack();
				}

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
		Column::rebuildPortals()
	{
		this->portals.clear();
		this->portalsByID.clear();

		const auto countX = this->parameters.arrangement.countX.get();
		const auto countY = this->parameters.arrangement.countY.get();

		int targetID = 1;
		for (int j = 0; j < countY; j++) {
			for (int i = 0; i < countX; i++) {
				auto portal = make_shared<Portal>(this->rs485, targetID);
				portal->onTargetChange += [this](Portal::Target) {
					this->portalsByIDDirty = true;
				};
				this->portals.push_back(portal);

				targetID++;
			}
		}
		this->portalsByIDDirty = true;
		ofxCvGui::refreshInspector(this);
	}

	//----------
	size_t Column::getCountX() const
	{
		return this->parameters.arrangement.countX.get();
	}


	//----------
	size_t Column::getCountY() const
	{
		return this->parameters.arrangement.countY.get();
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
	ofxCvGui::PanelPtr
		Column::getMiniView(float width)
	{
		auto countx = (float)this->getCountX();
		auto county = (float)this->getCountY();
		
		auto portals = this->getAllPortals();

		auto element = ofxCvGui::Panels::makeBlank();
		const auto cellSize = width / countx;
		element->setWidth(width);
		element->setHeight(county * cellSize);

		element->onDraw += [this](ofxCvGui::DrawArguments& args) {
			auto countx = (float)this->getCountX();
			auto county = (float)this->getCountY();
			const auto cellSize = args.localBounds.width / countx;

			auto portals = this->getAllPortals();

			// start at bottom left
			float y = cellSize * county;
			float x = 0.0f;
			int index = 0;
			const auto radius = cellSize / 2.0f - 3.0f;

			ofPushMatrix();
			{
				if (this->parameters.arrangement.flipped.get()) {
					auto height = cellSize * county;
					auto width = cellSize * countx;
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

					if (index % this->getCountX() == 0) {
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