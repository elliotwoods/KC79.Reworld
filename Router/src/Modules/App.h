#pragma once

#include "Base.h"
#include "crow/crow.h"

#include "Column.h"
#include "TestPattern.h"
#include "PatternPlayer.h"

#include "ofxOsc.h"
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
		void update() override;
		void populateInspector(ofxCvGui::InspectArguments& args);

		void load();
		void deserialise(const nlohmann::json&);

		vector<shared_ptr<Column>> getAllColumns() const;
		shared_ptr<Column> getColumnByID(int) const;

		glm::tvec2<size_t> getSize() const;
		void moveGrid(const vector<glm::vec2>& positions);
		void moveGridRow(const vector<glm::vec2>& positions, int rowIndex);

		void pollAll();
		void broadcast(const msgpack11::MsgPack&);
		void uploadFWAll(const string& path);

		ofxCvGui::ElementPtr getPositionsPreview();
	protected:
		static shared_ptr<App> instance;

		void setupCrowRoutes();
		crow::SimpleApp crow;
		std::future<void> crowRun;

		map<int, shared_ptr<Column>> columns;

		shared_ptr<TestPattern> testPattern;
		shared_ptr<PatternPlayer> patternPlayer;
		vector<shared_ptr<Base>> modules;

		struct : ofParameterGroup {
			struct : ofParameterGroup {
				ofParameter<bool> enabled{ "Enabled", true };
				ofParameter<int> port{ "Port", 4000 };
				PARAM_DECLARE("OSC", enabled, port);
			} osc;
			PARAM_DECLARE("App", osc);
		} parameters;

		shared_ptr<ofxOscReceiver> oscReceiver;
		ofxTCPServer tcpServer;


		struct {
			ofColor normal;
			ofColor flash{ 200, 200, 200 };
		} flashScreenSettings;
	};
}