#include "pch_App.h"
#include "Renderer.h"
#include "Sources/Gradient.h"
#include "Sources/FilePlayer.h"
#include "Sources/Text.h"

namespace Modules {
	namespace Image {
		//----------
		Renderer::Renderer()
		{
		}

		//----------
		string
			Renderer::getTypeName() const
		{
			return "Renderer";
		}

		//----------
		void
			Renderer::init()
		{
			this->sources.push_back(make_shared<Sources::Gradient>());
			this->sources.push_back(make_shared<Sources::FilePlayer>());
			this->sources.push_back(make_shared<Sources::Text>());
			for (auto source : this->sources) {
				source->init();
			}
		}

		//----------
		void
			Renderer::update()
		{
			for (auto source : this->sources) {
				source->update();
			}
		}

		//----------
		void
			Renderer::render(const Sources::RenderSettings& renderSettings)
		{
			// Render the individual images
			{
				for (auto source : this->sources) {
					source->allocate(renderSettings);
					source->render(renderSettings);
				}
			}

			// Render the total image
			{
				// Check if needs allocate
				if (this->pixels.getWidth() != renderSettings.width || this->pixels.getHeight() != renderSettings.height) {
					this->pixels.allocate(renderSettings.width, renderSettings.height, 3);
					this->preview.allocate(renderSettings.width, renderSettings.height, GL_RGB);
					this->preview.setTextureMinMagFilter(GL_NEAREST, GL_NEAREST);
				}

				// Clear the image (set to 0's)
				this->pixels.set(0.0f);

				// Sum the individual images into the result
				for (auto source : this->sources) {
					auto sourcePixels = source->pixels.getData();
					auto resultPixels = this->pixels.getData();
					for (int i = 0; i < this->pixels.size(); i++) {
						resultPixels[i] += sourcePixels[i];
					}
				}
			}

			// Update the previews
			{
				for (auto source : this->sources) {
					source->updatePreview();
				}

				this->preview.loadData(this->pixels);
			}
		}

		//----------
		void
			Renderer::populateInspector(ofxCvGui::InspectArguments& args)
		{

		}

		//----------
		void
			Renderer::deserialise(const nlohmann::json& json)
		{

		}

		//----------
		ofxCvGui::PanelPtr
			Renderer::getMiniView()
		{
			auto view = ofxCvGui::Panels::makeTexture(this->preview);
			return view;
		}

		//----------
		ofxCvGui::PanelPtr
			Renderer::getPanel()
		{
			auto panel = ofxCvGui::Panels::makeWidgets();

			for (auto source : this->sources) {
				panel->add(source->getButton(source));

			}
			return panel;
		}
	}
}