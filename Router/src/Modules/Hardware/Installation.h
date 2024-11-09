#pragma once

#include "../Base.h"

#include "Column.h"

namespace Modules {
	namespace Hardware {
		class Installation : public Base
		{
		public:
			Installation();
			~Installation();

			string getTypeName() const override;
			void init() override;
			void update() override;
			void populateInspector(ofxCvGui::InspectArguments& args);

			void deserialise(const nlohmann::json&);

			vector<shared_ptr<Column>> getAllColumns() const;
			shared_ptr<Column> getColumnByID(int) const;

			vector<shared_ptr<Portal>> getAllPortals() const;
			shared_ptr<Portal> getPortalByTargetID(int columnID, Portal::Target) const;

			glm::tvec2<size_t> getResolution() const; // columns, rows

			void pollAll();
			void broadcast(const msgpack11::MsgPack&);

			ofxCvGui::PanelPtr getMiniView();
		protected:
			map<int, shared_ptr<Column>> columns;
		};
	}
}