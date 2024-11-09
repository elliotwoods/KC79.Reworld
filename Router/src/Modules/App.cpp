#include "pch_App.h"
#include "App.h"
#include "../Utils.h"
#include "../OSC/Routes.h"

using namespace msgpack11;

namespace Modules {
	//----------
	shared_ptr<App> App::instance = nullptr;

	//----------
	App::App()
	{

	}

	//----------
	shared_ptr<App>
		App::X()
	{
		// Setup a singleton instance of the app that can be accessed from anywhere
		if (!App::instance) {
			App::instance = shared_ptr<App>(new App());
		}
		return App::instance;
	}

	//----------
	App::~App()
	{
		
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
		// Inspector things
		{
			// Universal Inspector widgets for all modules (presume we only init the app once)
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

			this->onPopulateInspector += [this](ofxCvGui::InspectArguments& args) {
				this->populateInspector(args);
				};
		}

		// Build the modules
		{
			this->renderer = make_shared<Image::Renderer>();
			this->installation = make_shared<Hardware::Installation>();
			this->oscReceiver = make_shared<OSC::Receiver>();
			this->restServer = make_shared<REST::Server>();

			// Add them to the modules list
			this->modules = {
				this->renderer
				, this->installation
				, this->oscReceiver
				, this->restServer
			};
		}

		// Initialise modules
		for(const auto& module : this->modules) {
			module->init();
		}


		// Load the config.json file
		this->load();
	}

	//----------
	void
		App::initGUI(ofxCvGui::Builder& gui)
	{
		auto strip1 = gui.addStrip();
		strip1->setCellSizes({ 100, -1 });
		{
			// Make a vertical strip for the left side
			auto modulesGrid = make_shared<ofxCvGui::Panels::Groups::Strip>(ofxCvGui::Panels::Groups::Strip::Direction::Vertical);
			{
				modulesGrid->add(this->renderer->getMiniView());
				modulesGrid->add(this->installation->getMiniView());
				modulesGrid->add(this->oscReceiver->getMiniView());
				modulesGrid->add(this->restServer->getMiniView());
			}
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
	}

	//----------
	void
		App::populateInspector(ofxCvGui::InspectArguments& args)
	{
		auto inspector = args.inspector;

		inspector->addFps();

		// Add modules
		for (const auto& module : this->modules) {
			module->addSubMenuToInsecptor(inspector, module);
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
		for (auto& module : this->modules) {
			auto moduleName = module->getName();
			if (json.contains(moduleName)) {
				module->deserialise(json[moduleName]);
			}
		}
	}

	//----------
	shared_ptr<Hardware::Installation>
		App::getInstallation()
	{
		return this->installation;
	}
}