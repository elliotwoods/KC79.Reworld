#include "pch_App.h"
#include "Base.h"

namespace Modules {
	namespace Image {
		namespace Sources {
			//----------
			Base::Base()
			{
				this->onPopulateInspector += [this](ofxCvGui::InspectArguments& args) {
					auto inspector = args.inspector;
					inspector->addParameterGroup(this->parameters);
				};
			}

			//----------
			void
				Base::allocate(const RenderSettings& renderSettings)
			{
				const auto width = renderSettings.width;
				const auto height = renderSettings.height;

				if (this->pixels.getWidth() != width || this->pixels.getHeight() != height) {
					this->pixels.allocate(width, height, 3);
					this->pixels.set(0);

					this->preview.allocate(width, height, GL_RGB);
					this->preview.setTextureMinMagFilter(GL_NEAREST, GL_NEAREST);
					this->preview.loadData(this->pixels);
				}
			}

			//----------
			void
				Base::updatePreview()
			{
				if (this->pixels.isAllocated()) {
					this->preview.loadData(this->pixels);
				}
				else {
					this->preview.clear();
				}
			}

			//----------
			shared_ptr<ofxCvGui::Widgets::Button>
				Base::getButton(shared_ptr<Base> this_shared)
			{
				auto button = make_shared<ofxCvGui::Widgets::SubMenuInspectable>(this->getName(), this_shared);
				button->setHeight(70.0f);

				// Draw the preview texture on the left side of the button with the correct aspect ratio
				button->onDraw += [this](ofxCvGui::DrawArguments& args) {
					// Outer bounds limit
					ofRectangle bounds(50, 3, 100, 66);

					// Crop the bounds to match the aspect ratio of the preview texture
					const auto aspectRatio = this->preview.getWidth() / (float)this->preview.getHeight();
					if (bounds.width / bounds.height > aspectRatio) {
						// Too wide
						bounds.width = bounds.height * aspectRatio;
					}
					else {
						// Too tall
						bounds.height = bounds.width / aspectRatio;
					}

					if (this->preview.isAllocated()) {
						this->preview.draw(bounds);
					}
					};

				// Add a button for toggling visibility
				{
					auto toggle = make_shared<ofxCvGui::Widgets::Toggle>(this->parameters.visible);
					toggle->setDrawGlyph(u8"\uf06e");
					toggle->setBounds({ 3, 3, 30, 30 });
					button->addChild(toggle);
				}

				// Add a button for toggling rendering
				{
					auto toggle = make_shared<ofxCvGui::Widgets::Toggle>(this->parameters.renderEnabled);
					toggle->setDrawGlyph(u8"\uf04b");
					toggle->setBounds({ 3, 36, 30, 30 });
					button->addChild(toggle);
				}

				// Add a slider for alpha
				{
					auto slider = ofxCvGui::makeElement();
					slider->addToolTip("Alpha");
					slider->setBounds({ 38, 3, 8, 66 });

					auto sliderWeak = weak_ptr<ofxCvGui::Element>(slider);
					slider->onDraw += [this, sliderWeak](ofxCvGui::DrawArguments& args) {
						auto slider = sliderWeak.lock();
						if (!slider) {
							return;
						}

						// Draw outline when mouse over
						if (slider->isMouseOver()) {
							ofPushStyle();
							{
								ofNoFill();
								ofDrawRectangle(args.localBounds);
							}
							ofPopStyle();
						}

						// Draw the slider
						auto y = (1.0f - this->parameters.alpha) * args.localBounds.height;
						ofDrawRectangle(0, y, args.localBounds.width, args.localBounds.height - y);
						};
					slider->onMouse += [this, sliderWeak](ofxCvGui::MouseArguments& args) {
						auto slider = sliderWeak.lock();
						if (!slider) {
							return;
						}

						if (args.takeMousePress(slider) || args.isDragging(slider)) {
							this->parameters.alpha.set(1.0f - ofClamp(args.localNormalized.y, 0.0f, 1.0f));
						}
						};
					button->addChild(slider);
				}

				// Draw an outline on the button if this source is being selected
				button->onDraw += [this_shared](ofxCvGui::DrawArguments& args) {
					if (ofxCvGui::isBeingInspected(this_shared)) {
						ofPushStyle();
						{
							ofNoFill();
							ofDrawRectangle(args.localBounds);
						}
						ofPopStyle();
					}
					};

				return button;
			}

			//----------
			void
				Base::deserialise(const nlohmann::json& json)
			{
				if (json.contains("visible")) {
					this->parameters.visible.set(json["visible"]);
				}
				if (json.contains("renderEnabled")) {
					this->parameters.renderEnabled.set(json["renderEnabled"]);
				}
			}

			//----------
			const Base::Parameters&
				Base::getBaseParameters() const
			{
				return this->parameters;
			}
		}
	}
}