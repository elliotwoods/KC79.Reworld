#pragma once

#include "../Base.h"
#include "msgpack11.hpp"

namespace Modules {
	class Portal;

	namespace PerPortal {
		class Logger : public Base
		{
		public:
			enum LogLevel : uint8_t {
				Status = 0
				, Warning = 10
				, Error = 20
			};

			struct LogMessage {
				std::string message;
				LogLevel level;
				float timetamp = 0;
				size_t count = 1;
			};

			class MessageElement : public ofxCvGui::Element {
			public:
				MessageElement(const LogMessage& logMessage);
				void draw(ofxCvGui::DrawArguments&);
				void mouse(ofxCvGui::MouseArguments&);
			protected:
				const LogMessage logMessage;
			};

			Logger(Portal*);
			string getTypeName() const override;
			string getGlyph() const override;

			void init();
			void update();
			void populateInspector(ofxCvGui::InspectArguments& args);
			ofxCvGui::PanelPtr getPanel();

			void processIncoming(const nlohmann::json&) override;

			void limitMaxMessages();
			void clear();

			const LogMessage* getLatestMessage() const;
		protected:
			void refreshPanels();
			void refreshPanel(shared_ptr<ofxCvGui::Panels::Widgets>);
			Portal* portal;
			vector<LogMessage> logMessages;

			vector<weak_ptr<ofxCvGui::Panels::Widgets>> panels;
			bool panelsStale = true;

			struct : ofParameterGroup {
				ofParameter<int> maxHistorySize{ "Max history size", 100 };
				ofParameter<bool> prioritiseErrors{ "Prioritise keep all errors", true };
				PARAM_DECLARE("Logger", maxHistorySize, prioritiseErrors);
			} parameters;
		};
	}
}