#include "pch_App.h"
#include "App.h"
#include "..\Utils.h"

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

		{
			for (int j = 0; j < 3; j++) {
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
		}
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

		// Actions
		inspector->addButton("Initialise all", [this]() {
			for (auto portal : this->portals) {
				portal->initRoutine();
			}
			})->setDrawGlyph(u8"\uf11e");
		inspector->addButton("See through all", [this]() {
			for (auto portal : this->portals) {
				portal->getPilot()->seeThrough();
			}
			})->setDrawGlyph(u8"\uf06e");
	}

	//----------
	void
		App::processIncoming(const nlohmann::json& json)
	{
		if (json.size() >= 3) {
			// It's a packet
			auto target = (uint8_t)json[0];
			auto origin = (uint8_t)json[1];
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
		App::refreshPortalsByID()
	{
		this->portalsByID.clear();
		for (auto portal : this->portals) {
			this->portalsByID.emplace(portal->getTarget(), portal);
		}
		this->portalsByIDDirty = false;
	}
}