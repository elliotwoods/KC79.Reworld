#pragma once

#include "Base.h"
#include "crow/crow.h"

#include "Image/Renderer.h"
#include "Hardware/Installation.h"
#include "OSC/Receiver.h"
#include "REST/Server.h"

#include "ofxNetwork.h"

namespace Modules {
	class App : public Base
	{
		App();
	public:
		static shared_ptr<App> X();
		~App();
		
		string getTypeName() const override;
		void init() override;
		void initGUI(ofxCvGui::Builder&);

		void update() override;
		void populateInspector(ofxCvGui::InspectArguments& args);

		void load();
		void deserialise(const nlohmann::json&);

		shared_ptr<Hardware::Installation> getInstallation();
	protected:
		static shared_ptr<App> instance;

		vector<shared_ptr<Modules::Base>> modules;
		shared_ptr<Image::Renderer> renderer;
		shared_ptr<Hardware::Installation> installation;
		shared_ptr<OSC::Receiver> oscReceiver;
		shared_ptr<REST::Server> restServer;
	};
}