#pragma once

#include "../TopLevelModule.h"

#include "Column.h"
#include "MassFWUpdate.h"

namespace Modules {
	namespace Hardware {
		class Installation : public TopLevelModule
		{
		public:
			MAKE_ENUM(ImageTransmit
				, (Inidividual, Keyframe, Disabled)
				, ("Inidividual", "Keyframe", "Disabled"));

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

			void takeImage();
			void transmitFrame();

			chrono::system_clock::duration getTransmitKeyframeInterval() const;
			int getTransmitKeyframeBatchSize() const;
			bool getKeyframeVelocitiesEnabled() const;

			void homeHardwareAndZeroPositions();
		protected:
			void rebuildPanel();

			vector<shared_ptr<Column>> columns;
			bool needsRebuildColumns = true;

			shared_ptr<MassFWUpdate> massFWUpdate;

			shared_ptr<ofxCvGui::Panels::Widgets> panel;
			bool needsRebuildPanel = true;

			struct : ofParameterGroup {
				struct : ofParameterGroup {
					ofParameter<ImageTransmit> transmit{ "Transmit", ImageTransmit::Inidividual };
					ofParameter<float> periodS{ "Period [s]", 0.5, 0, 10 };
					ofParameter<int> keyframeBatchSize{ "Keyframe batch size", 8 };
					ofParameter<bool> keyframeVelocities{ "Keyframe velocities", true };
					PARAM_DECLARE("Messaging", transmit, periodS, keyframeBatchSize, keyframeVelocities);
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