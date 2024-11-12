#pragma once

#include "../TopLevelModule.h"

#include "Column.h"

namespace Modules {
	namespace Hardware {
		class Installation : public TopLevelModule
		{
		public:
			Installation();
			~Installation();

			string getTypeName() const override;
			void init() override;
			void update() override;
			void populateInspector(ofxCvGui::InspectArguments& args);

			void deserialise(const nlohmann::json&);

			void rebuildColumns();

			vector<shared_ptr<Column>> getAllColumns() const;
			shared_ptr<Column> getColumnByID(size_t) const;

			vector<shared_ptr<Portal>> getAllPortals() const;
			shared_ptr<Portal> getPortalByTargetID(size_t columnID, Portal::Target) const;

			glm::tvec2<size_t> getResolution() const; // columns, rows

			void pollAll();
			void broadcast(const msgpack11::MsgPack&);

			ofxCvGui::PanelPtr getPanel() override;
			ofxCvGui::PanelPtr getMiniView() override;
		protected:
			void rebuildPanel();
			void rebuildMiniView();

			vector<shared_ptr<Column>> columns;

			shared_ptr<ofxCvGui::Panels::Groups::Strip> miniView;
			shared_ptr<ofxCvGui::Panels::Widgets> panel;
			bool needsRebuildMiniView = true;
			bool needsRebuildPanel = true;

			struct : ofParameterGroup {
				struct : ofParameterGroup {
					ofParameter<int> columns{ "Columns", 32 };
					ofParameter<int> rows{ "Rows", 24 };
					ofParameter<int> columnWidth{ "Column width", 1 };
					ofParameter<bool> flipped{ "Flipped", false };
					PARAM_DECLARE("Arrangement", columns, rows, columnWidth, flipped);
				} arrangement;

				PARAM_DECLARE("Installation", arrangement);
			} parameters;
		};
	}
}