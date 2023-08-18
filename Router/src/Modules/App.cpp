#include "pch_App.h"
#include "App.h"


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
			this->portal = make_shared<Portal>(this->rs485, 1);
			this->modules.push_back(this->portal);
			this->portalByID.emplace(1, this->portal);
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
		for (auto module : this->modules) {
			module->update();
		}
	}

	//----------
	void
		App::populateInspector(ofxCvGui::InspectArguments& args)
	{
		auto inspector = args.inspector;

		inspector->addFps();

		for (auto module : this->modules) {
			module->addSubMenuToInsecptor(inspector, module);
		}
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
				for (auto& it : this->portalByID) {
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
}