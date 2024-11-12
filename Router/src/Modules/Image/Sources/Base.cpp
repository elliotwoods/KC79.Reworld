#include "pch_App.h"
#include "Base.h"

namespace Modules {
	namespace Image {
		namespace Sources {
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
				this->preview.loadData(this->pixels);
			}

			//----------
			shared_ptr<ofxCvGui::Widgets::Button>
				Base::getButton(shared_ptr<Base> this_shared)
			{
				auto button = make_shared<ofxCvGui::Widgets::SubMenuInspectable>(this->getName(), this_shared);
				button->onDraw += [this](ofxCvGui::DrawArguments& args) {
					// Draw the preview texture on the left side of the button with the correct aspect ratio

					// Outer bounds limit
					ofRectangle bounds(20, 20, 100, args.localBounds.height - 40);

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

					this->preview.draw(bounds);
					};

				return button;
			}
		}
	}
}