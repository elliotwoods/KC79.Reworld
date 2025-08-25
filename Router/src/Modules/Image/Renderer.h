#pragma once

#include "../TopLevelModule.h"
#include "Sources/Base.h"

namespace Modules {
	namespace Image {
		class Renderer : public TopLevelModule
		{
		public:
			Renderer();

			string getTypeName() const override;
			void init() override;
			void update() override;

			void render(const Sources::RenderSettings&);

			void populateInspector(ofxCvGui::InspectArguments&);
			void deserialise(const nlohmann::json&);

			ofxCvGui::PanelPtr getMiniView() override;
			ofxCvGui::PanelPtr getPanel() override;

			const ofFloatPixels& getPixels() const;
		protected:
			void addSource();
			void refreshPanel();
			vector<shared_ptr<Sources::Base>> sources;
			ofFloatPixels pixels;
			ofTexture preview;

			shared_ptr<ofxCvGui::Panels::Widgets> panel;
			bool needsPanelRefresh = true;
		};
	}
}