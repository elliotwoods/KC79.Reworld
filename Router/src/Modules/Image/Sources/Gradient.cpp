#include "pch_App.h"
#include "Gradient.h"

namespace Modules {
	namespace Image {
		namespace Sources {
			//----------
			Gradient::Gradient()
			{
				
			}

			//----------
			string
				Gradient::getTypeName() const
			{
				return "Image::Gradient";
			}

			//----------
			void
				Gradient::init()
			{
				this->onPopulateInspector += [this](ofxCvGui::InspectArguments& args) {
					this->populateInspector(args);
					};
			}

			//----------
			void
				Gradient::populateInspector(ofxCvGui::InspectArguments& args)
			{
				auto inspector = args.inspector;
				inspector->addParameterGroup(this->parameters);
			}

			//----------
			void
				Gradient::render(const RenderSettings& renderSettings)
			{
				auto data = this->pixels.getData();

				// Calculate the gradient
				for (int j = 0; j < renderSettings.height; j++) {
					for (int i = 0; i < renderSettings.width; i++) {
						auto xy = glm::vec2(i, j) - glm::vec2(renderSettings.width, renderSettings.height) / 2;
						xy /= (float) min(renderSettings.width, renderSettings.height) / 2;

						auto r = glm::length(xy);
						
						// Calculate the gradient
						float thi = 0.0f;
						{
							switch (this->parameters.gradientType.get()) {
							case GradientType::Radial:
								thi = r;
								break;
							case GradientType::Horizontal:
								thi = xy.x;
								break;
							case GradientType::Vertical:
								thi = xy.y;
								break;
							default:
								break;
							}
						}

						// Apply a time element
						{
							thi -= this->parameters.speed * renderSettings.time;
						}

						// Ensure positive values
						thi = fabs(thi);

						// Calculate the wave
						float alpha;
						{
							switch (this->parameters.wave.get()) {
							case Wave::Triangle:
								alpha = fmod(thi * this->parameters.frequency, 2.0f);
								if (alpha > 1.0f) {
									alpha = 2.0f - thi;
								}
								break;
							case Wave::Sine:
								alpha = sin(thi * this->parameters.frequency * PI) * 0.5f + 0.5f;
								break;
							case Wave::Sawtooth:
								alpha = fmod(thi * this->parameters.frequency, 1.0f);
								break;
							default:
								break;
							}
						}

						*data++ = ofMap(alpha, 0.0f, 1.0f, this->parameters.value1.get().x, this->parameters.value2.get().x);
						*data++ = ofMap(alpha, 0.0f, 1.0f, this->parameters.value1.get().y, this->parameters.value2.get().y);
					}
				}
			}
		}
	}
}