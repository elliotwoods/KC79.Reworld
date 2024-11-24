#include "pch_App.h"
#include "Column.h"
#include "../App.h"

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
		Column::pushStale(bool useKeyframe)
	{
		if (!useKeyframe) {
			// Check them individually and just push the ones that are stale
			for (auto portal : this->portals) {
				if (portal->getPilot()->needsPush()) {
					portal->getPilot()->pushLazy();
				}
			}
		}
		else {
			// If using keyframe, check if any are stale
			bool isStale = false;
			for (auto portal : this->portals) {
				if (portal->getPilot()->needsPush()) {
					isStale = true;
					break;
				}
			}

			if (isStale) {
				this->transmitKeyframe();
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
					this->broadcast(action.message, false);
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
							}, true);
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

		this->countX = countX;
		this->countY = countY;

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
		Column::broadcast(const msgpack11::MsgPack& message, bool collateable)
	{
		auto packet = RS485::Packet(
			msgpack11::MsgPack::array{
				-1
				, (int8_t)0
				, message
			});
		packet.needsACK = false;
		packet.collateable = collateable;
		this->rs485->transmit(packet);
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
		Column::updatePositionsFromImage(const ofFloatPixels& pixels)
	{
		// For logging
		string moduleName = "Column " + ofToString(this->columnIndex) + "::updatePositionsFromImage";

		// Check we have the correct number of local portals
		if (this->portals.size() != this->countX * this->countY) {
			ofLogError(moduleName) << "Portals not allocated correctly";
			return;
		}

		// Get target position vectors from the pixels
		vector<glm::vec2> targetPositions;
		{
			// Check that the pixels is the correct resolution
			{
				if (pixels.getWidth() < (this->columnIndex + 1) * this->countX) {
					ofLogError(moduleName) << "Image resolution is not wide enough for this column";
					return;
				}
				if (pixels.getHeight() < this->countY) {
					ofLogError(moduleName) << "Image resolution is not tall enough for this column";
					return;
				}
			}

			// Get the pixels for this column
			{
				const auto flipped = this->parameters.arrangement.flipped.get();
				const auto data = (glm::vec3*)pixels.getData();
				const auto pixelWidth = pixels.getWidth();

				for (int j = 0; j < this->countY; j++) {
					for (int i = 0; i < this->countX; i++) {
						auto x = this->columnIndex * this->countX + i;
						auto y = j;

						if (!flipped) {
							// Default is bottom to top indexed
							y = this->countY - 1 - j;
						}


						const auto targetPosition = data[x + y * pixelWidth];

						auto pilot = this->portals[i + j * this->countX]->getPilot();
						pilot->setPosition(targetPosition);
						pilot->update(); // calculate the other values with the new position

					}
				}
			}
		}
	}

	//----------
	void
		Column::transmitKeyframe()
	{
		if (!this->getRS485()->isConnected()) {
			return;
		}

		// For logging
		string moduleName = "Column " + ofToString(this->columnIndex) + "::transmitKeyframe";

		// Get target position vectors from the portals (already update from pixels before this stage)
		vector<glm::vec2> targetPositions;
		{
			for(auto portal : this->portals) {
				targetPositions.push_back(portal->getPilot()->getPosition());
			}
		}

		// Gather the axis values
		vector<glm::vec2> axisValues;
		{
			for (size_t i = 0; i < this->portals.size(); i++) {
				auto pilot = this->portals[i]->getPilot();
				pilot->notifyValuesSent();
				axisValues.push_back(pilot->getAxisSteps());
			}
		}

		// Calculate velocities
		vector<glm::vec2> velocities;
		{
			if (this->lastKeyframe.axisValues.size() == axisValues.size()) {
				auto now = chrono::system_clock::now();

				// get dt
				auto dt = now - this->lastKeyframe.lastKeyframeTime;
				this->lastKeyframe.lastKeyframeTime = now;

				// cast to seconds
				auto dt_s = (float) std::chrono::duration_cast<std::chrono::milliseconds>(dt).count() / 1000.0f;

				for (size_t i = 0; i < this->portals.size(); i++) {
					velocities.push_back((axisValues[i] - this->lastKeyframe.axisValues[i]) / dt_s);
				}
			}
			else {
				// Fill with zeros
				velocities.resize(this->portals.size(), glm::vec2(0.0f, 0.0f));
			}
		}

		// Transmit keyframe message (in blocks)
		{
			auto maxBlockSize = App::X()->getInstallation()->getTransmitKeyframeBatchSize();

			size_t blockStartIndex = 1;

			msgpack11::MsgPack::array keyframeValues;

			auto transmitBlock = [&]() {
				this->broadcast(msgpack11::MsgPack::object{
						{
							"keyframe"
							, msgpack11::MsgPack::object {
								{ "startIndex",  blockStartIndex }
								, { "values", keyframeValues }
							}
						}
					}, false);
				blockStartIndex += keyframeValues.size();
				keyframeValues.clear();
				};

			for (size_t i = 0; i < this->portals.size(); i++) {
				keyframeValues.push_back(msgpack11::MsgPack::array{
					(int32_t) axisValues[i].x
					, (int32_t)axisValues[i].y
					, (int32_t)velocities[i].x
					, (int32_t)velocities[i].y
					});

				if (keyframeValues.size() >= maxBlockSize) {
					// Transmit invididual blocks and clear the keyframe
					transmitBlock();
				}
			}

			if (!keyframeValues.empty()) {
				// Transmit last block
				transmitBlock();
			}
		}

		// Store this frame as previous keyframe with timecode
		{
			this->lastKeyframe.axisValues = axisValues;
		}
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