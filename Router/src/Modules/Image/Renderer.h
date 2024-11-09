#pragma once

#include "../Base.h"

namespace Modules {
	namespace Image {
		class Renderer : public Base
		{
		public:
			Renderer();

			string getTypeName() const override;
			void init() override;
			void update() override;

			void populateInspector(ofxCvGui::InspectArguments&);
			void deserialise(const nlohmann::json&);

			ofxCvGui::PanelPtr getMiniView();
		protected:
		};
	}
}