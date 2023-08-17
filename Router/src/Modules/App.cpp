#include "pch_App.h"
#include "App.h"


namespace Modules {
	//----------
	App::App()
	{
		{
			this->rs485 = make_shared<RS485>();
			this->modules.push_back(this->rs485);
		}

		{
			this->fwUpdate = make_shared<FWUpdate>(this->rs485);
			this->modules.push_back(this->fwUpdate);
		}

		{
			this->portal = make_shared<Portal>(this->rs485, 1);
			this->modules.push_back(this->portal);
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
		App::dragEvent(const ofDragInfo& dragInfo)
	{
		for (const auto& file : dragInfo.files) {
			this->fwUpdate->uploadFirmware(file);
		}
	}
}