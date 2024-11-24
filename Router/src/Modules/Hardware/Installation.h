#pragma once

#include "../TopLevelModule.h"

#include "Column.h"

namespace Modules {
	namespace Hardware {
		class Installation : public TopLevelModule
		{
		public:
			MAKE_ENUM(ImageTransmit
				, (Keyframe, Inidividual)
				, ("Keyframe", "Individual"));

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
			void broadcast(const msgpack11::MsgPack&, bool collateable);

			ofxCvGui::PanelPtr getPanel() override;
			ofxCvGui::PanelPtr getMiniView() override;

			void transmitKeyframe();

			chrono::system_clock::duration getTransmitKeyframeInterval() const;
			int getTransmitKeyframeBatchSize() const;
		protected:
			void rebuildPanel();
			void rebuildMiniView();

			vector<shared_ptr<Column>> columns;
			bool needsRebuildColumns = true;

			shared_ptr<ofxCvGui::Panels::Groups::Strip> miniView;
			shared_ptr<ofxCvGui::Panels::Widgets> panel;
			bool needsRebuildMiniView = true;
			bool needsRebuildPanel = true;

			struct : ofParameterGroup {
				struct : ofParameterGroup {
					ofParameter<ImageTransmit> transmit{ "Transmit", ImageTransmit::Keyframe };
					ofParameter<int> keyframeBatchSize{ "Keyframe batch size", 8 };
					ofParameter<float> periodS{ "Period [s]", 0.1, 0, 10 };
					PARAM_DECLARE("Messaging", transmit, periodS, keyframeBatchSize);
				} messaging;

				struct : ofParameterGroup {
					ofParameter<bool> enabled{ "Enabled", true };
					PARAM_DECLARE("Image", enabled)
				} image;

				struct : ofParameterGroup {
					ofParameter<int> columns{ "Columns", 32 };
					ofParameter<int> rows{ "Rows", 24 };
					ofParameter<int> columnWidth{ "Column width", 1 };
					ofParameter<bool> flipped{ "Flipped", false };
					PARAM_DECLARE("Arrangement", columns, rows, columnWidth, flipped);
				} arrangement;

				PARAM_DECLARE("Installation", messaging, image, arrangement);
			} parameters;

			chrono::system_clock::time_point lastTransmitKeyframe = chrono::system_clock::now();
		};
	}
}