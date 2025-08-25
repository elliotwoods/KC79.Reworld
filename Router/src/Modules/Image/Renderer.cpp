#include "pch_App.h"
#include "Renderer.h"
#include "Sources/Factory.h"

namespace Modules {
	namespace Image {
		//----------
		Renderer::Renderer()
		{
			this->panel = ofxCvGui::Panels::makeWidgets();
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
			this->onPopulateInspector += [this](ofxCvGui::InspectArguments& args) {
				this->populateInspector(args);
				};
		}

		//----------
		void
			Renderer::update()
		{
			for (auto source : this->sources) {
				source->update();
			}
			if (this->needsPanelRefresh) {
				this->refreshPanel();
			}
		}

		//----------
		void
			Renderer::render(const Sources::RenderSettings& renderSettings)
		{
			// Render the individual images
			{
				for (auto source : this->sources) {
					const auto& baseParameters = source->getBaseParameters();
					if (baseParameters.renderEnabled.get()) {
						source->allocate(renderSettings);
						source->render(renderSettings);
					}
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
					const auto& baseParameters = source->getBaseParameters();

					// If it's not visible or not allocated correctly (e.g. hasn't been rendered)
					if (!baseParameters.visible.get()
						|| source->pixels.size() != this->pixels.size()) {
						continue;
					}

					auto sourcePixels = source->pixels.getData();
					auto resultPixels = this->pixels.getData();
					const auto& alpha = baseParameters.alpha.get();

					auto width = this->pixels.getWidth();
					auto height = this->pixels.getHeight();

					switch (baseParameters.style.get()) {
					case Sources::Style::Direct:
					{
						// Simply add the pixel values
						for (int i = 0; i < this->pixels.size(); i++) {
							resultPixels[i] += sourcePixels[i] * alpha;
						}
						break;
					}
					case Sources::Style::HV_ThetaR:
					{
						// Interpret HV as theta-R
						auto pixelCount = this->pixels.getWidth() * this->pixels.getHeight();
						auto input = (glm::vec3*)source->pixels.getData();
						auto output = (glm::vec3*)resultPixels;

						for (size_t i = 0; i < pixelCount; i++) {
							const auto& in = input[i];
							ofFloatColor color(in.x, in.y, in.z);
							float hue, saturation, brightness;
							color.getHsb(hue, saturation, brightness);
							auto& out = output[i];
							auto r = brightness;
							auto theta = hue;
							out.x += cos(theta) * r;
							out.y += sin(theta) * r;
						}

						break;
					}
					case Sources::Style::Centered:
					{
						// Interpret V as R and theta is always away from center

						auto pixelCount = this->pixels.getWidth() * this->pixels.getHeight();
						auto input = (glm::vec3*)source->pixels.getData();
						auto output = (glm::vec3*)resultPixels;

						auto halfWidth = width / 2;
						auto halfHeight = height / 2;

						for (size_t j = 0; j < height; j++) {
							for (size_t i = 0; i < width; i++) {

								glm::vec2 x{ i - halfWidth, j - halfHeight };
								auto theta = atan2(x.y, x.x);

								const auto& in = input[i];
								ofFloatColor color(in.x, in.y, in.z);
								float hue, saturation, brightness;
								color.getHsb(hue, saturation, brightness);

								auto& out = output[i + j * width];

								auto r = glm::length(x) / max(halfWidth, halfHeight);
								r *= brightness;

								out.x += cos(theta) * r;
								out.y += sin(theta) * r;
							}
						}
					}
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
			auto inspector = args.inspector;
		}

		//----------
		void
			Renderer::deserialise(const nlohmann::json& json)
		{
			if (json.contains("sources")) {
				const auto& jsonSources = json["sources"];
				if (jsonSources.is_array()) {
					for (const auto& jsonSource : jsonSources) {
						auto source = Sources::createFromJson(jsonSource);
						if (source) {
							source->init();
							this->sources.push_back(source);
						}
					}
				}
			}
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
			return this->panel;
		}

		//----------
		const ofFloatPixels&
			Renderer::getPixels() const
		{
			return this->pixels;
		}

		//----------
		void
			Renderer::addSource()
		{
			auto selectPanel = ofxCvGui::Panels::makeWidgets();
			{
				for (const auto& factory : Sources::factories) {
					selectPanel->addButton(factory.typeName, [this, factory]() {
						auto module = factory.createModule({});
						module->init();
						this->sources.push_back(module);
						this->needsPanelRefresh = true;
						ofxCvGui::closeDialog();
						});
				}
			};
			ofxCvGui::openDialog(selectPanel);
		}

		//----------
		void 
			Renderer::refreshPanel()
		{
			this->panel->clear();

			for (auto source : this->sources) {
				this->panel->add(source->getButton(source));
			}

			this->panel->addButton("+", [this]() {
				this->addSource();
				});
			this->needsPanelRefresh = false;
		}
	}
}