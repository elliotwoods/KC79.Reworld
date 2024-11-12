#include "pch_App.h"
#include "Text.h"

namespace Modules {
	namespace Image {
		namespace Sources {
			//----------
			Text::Text()
			{

			}

			//----------
			string
				Text::getTypeName() const
			{
				return "Image::Sources::Text";
			}

			//----------
			void
				Text::init()
			{
				this->onPopulateInspector += [this](ofxCvGui::InspectArguments& args) {
					this->populateInspector(args);
					};
			}

			//----------
			void
				Text::update()
			{
				auto& fonts = ofxAssets::AssetRegister().getFonts();

				// Check any fonts exist
				if (fonts.empty()) {
					return;
				}

				// Check selected font exists
				{
					auto findFont = fonts.find(this->parameters.font.get());
					if (findFont == fonts.end()) {
						// Choose the first font in asset registry otherwise
						this->parameters.font.set(fonts.begin()->first);
					}
				}
			}

			//----------
			void
				Text::populateInspector(ofxCvGui::InspectArguments& args)
			{
				auto inspector = args.inspector;
				inspector->addParameterGroup(this->parameters);

				{
					auto fontSelectionPanel = ofxCvGui::Panels::makeWidgets();
					{
						const auto & fonts = ofxAssets::AssetRegister().getFonts();
						for (const auto& it: fonts) {
							const auto& name = it.first;
							auto button = fontSelectionPanel->addButton(name, [this, name]() {
								this->parameters.font.set(name);
								ofxCvGui::closeDialog();
								});

							button->setHeight(60.0f);

							button->onDraw += [this, name](ofxCvGui::DrawArguments& args) {
								const auto height = 20;
								const auto& font = ofxAssets::font(name, height);
								font.drawString(name, args.localBounds.x, args.localBounds.y + (args.localBounds.height - height) / 2);
								};
						}
					}
					inspector->addButton("Select font...", [this, fontSelectionPanel]() {
						ofxCvGui::openDialog(fontSelectionPanel);
						})->setHeight(100.0f);
				}
				
			}

			//----------
			void
				Text::render(const RenderSettings& renderSettings)
			{
				// check the fbo is allocated correctly
				if (this->fbo.getWidth() != renderSettings.width || this->fbo.getHeight() != renderSettings.height) {
					this->fbo.allocate(renderSettings.width, renderSettings.height, GL_RGB);
				}

				// render the text into fbo
				{
					this->fbo.begin();
					{
						if (!this->parameters.inverse) {
							ofClear(0, 0);
							ofSetColor(255);
						}
						else {
							ofClear(255);
							ofSetColor(0);
						}

						const auto& border = this->parameters.border.get();
						const auto& text = this->parameters.text.get();

						// Calculate target bounds that the text should fit within
						ofRectangle targetBounds(border, border, renderSettings.width - border * 2, renderSettings.height - border * 2);

						if (this->parameters.usePixelFont) {
							auto naturalBounds = ofRectangle(0, -11, text.size() * 8, 11);
							const auto offset = targetBounds.getCenter() - naturalBounds.getCenter();

							// draw as bitmap string
							ofPushMatrix();
							{
								ofTranslate(offset);
								ofDrawBitmapString(text, 0, 0);
							}
							ofPopMatrix();
						}
						else {
							if (this->loadedFontName != this->parameters.font.get()
								|| this->loadedFontSize != this->parameters.size.get()) {
								const auto & fontAsset = ofxAssets::AssetRegister().getFonts()[this->parameters.font.get()];
								auto filename = fontAsset->getFilename();
								this->font.loadFont(filename, this->parameters.size.get(), false, true, false);

								this->loadedFontName = this->parameters.font.get();
								this->loadedFontSize = this->parameters.size.get();
							}
							// Calculate the natural (untransformed) text bounds
							auto naturalBounds = this->font.getStringBoundingBox(text, 0, 0);

							// Calculate the scale and offset to fit the text within the target bounds
							const auto scale = std::min(targetBounds.width / naturalBounds.width, targetBounds.height / naturalBounds.height);
							const auto offset = targetBounds.getCenter() - naturalBounds.getCenter() * scale;

							// Draw the text with scale and offset
							ofPushMatrix();
							{
								ofTranslate(offset);
								ofScale(scale, scale);
								this->font.drawString(text, 0, 0);
							}
							ofPopMatrix();
						}
					}
					this->fbo.end();
				}

				this->fbo.readToPixels(this->pixels);
			}
		}
	}
}