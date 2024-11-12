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
		this->placeholderView = make_shared<ofxCvGui::Panels::Groups::Grid>();

		auto strip1 = gui.addStrip();
		strip1->setCellSizes({ 150, -1, 300 });
		{
			// Make a vertical strip for the left side
			{
				auto topLevelModulesStrip = make_shared<ofxCvGui::Panels::Groups::Strip>(ofxCvGui::Panels::Groups::Strip::Direction::Vertical);
				{
					vector<int> cellSizes;

					for (auto submodule : this->modules) {
						auto miniView = submodule->getMiniView();
						topLevelModulesStrip->add(miniView);
						miniView->onMouseReleased += [submodule, this](ofxCvGui::MouseArguments& args) {
							// Set the central panel
							{
								auto panel = submodule->getPanel();
								this->placeholderView->clear();
								if (panel) {
									this->placeholderView->add(panel);
								}
								else {
									this->placeholderView->add(make_shared<ofxCvGui::Panels::Text>("No panel available"));
								}
							}

							// Set the inspector
							{
								ofxCvGui::inspect(submodule);
							}
							};

						// Draw an outline on selected views
						miniView->onDraw += [submodule, this](ofxCvGui::DrawArguments& args) {
							auto isBeingInspected = ofxCvGui::isBeingInspected(submodule);
							
							ofxCvGui::Utils::drawText(submodule->getName()
								, 0, 0
								, true
								, 10, 10
								, false
								, isBeingInspected ? ofColor(255) : ofColor(40));

							if (isBeingInspected) {
								ofDrawLine(args.localBounds.getTopLeft(), args.localBounds.getTopRight());
								ofDrawLine(args.localBounds.getTopLeft(), args.localBounds.getBottomLeft());
								ofDrawLine(args.localBounds.getBottomLeft(), args.localBounds.getBottomRight());
							}
							};

						cellSizes.push_back(submodule->getMiniViewHeight());
					}

					topLevelModulesStrip->setCellSizes(cellSizes);
				}
				strip1->add(topLevelModulesStrip);
			}

			// Add the placeholder to center
			{
				strip1->add(placeholderView);
			}

			// Add an inspector for RHS
			{
				auto inspectorPanel = ofxCvGui::Panels::makeInspector();
				inspectorPanel->setTitleEnabled(false);
				strip1->add(inspectorPanel);
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

		// Render images
		{
			Image::Sources::RenderSettings renderSettings;
			{
				const auto resolution = this->installation->getResolution();
				renderSettings.width = resolution.x;
				renderSettings.height = resolution.y;
				renderSettings.time = ofGetElapsedTimef();
			}
			this->renderer->render(renderSettings);
		}

		if (this->renderer->isTransmitEnabled()) {
			// Transmit the image to the hardware
			{
				const auto & pixels = this->renderer->getPixels();
			}
		}
	}

	//----------
	void
		App::populateInspector(ofxCvGui::InspectArguments& args)
	{
		auto inspector = args.inspector;

		inspector->addFps();
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