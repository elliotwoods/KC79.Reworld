#pragma once

#include "Base.h"
#include "crow/crow.h"

#include "Column.h"
#include "ofxOsc.h"

namespace Modules {
	class App : public Base
	{
	public:
		App();
		~App();
		
		string getTypeName() const override;
		void init() override;
		void update() override;
		void populateInspector(ofxCvGui::InspectArguments& args);

		shared_ptr<Column> getColumnByID(int) const;

		void dragEvent(const ofDragInfo&);
	protected:
		void setupCrowRoutes();
		crow::SimpleApp crow;
		std::future<void> crowRun;

		map<int, shared_ptr<Column>> columns;

		struct : ofParameterGroup {
			struct : ofParameterGroup {
				ofParameter<bool> enabled{ "Enabled", true };
				ofParameter<int> port{ "Port", 4000 };
				PARAM_DECLARE("App", enabled, port);
			} osc;
			PARAM_DECLARE("OSC", osc);
		} parameters;

		shared_ptr<ofxOscReceiver> oscReceiver;
	};
}