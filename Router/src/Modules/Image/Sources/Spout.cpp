#include "pch_App.h"
#include "Spout.h"

namespace Modules {
	namespace Image {
		namespace Sources {
			//----------
			Spout::Spout()
			{

			}

			//----------
			string
				Spout::getTypeName() const
			{
				return "Image::Sources::Spout";
			}

			//----------
			void
				Spout::init()
			{
				this->onPopulateInspector += [this](ofxCvGui::InspectArguments& args) {
					this->populateInspector(args);
					};

				this->spoutReceiver.init();
			}

			//----------
			void
				Spout::update()
			{

			}

			//----------
			void
				Spout::populateInspector(ofxCvGui::InspectArguments& args)
			{
				auto inspector = args.inspector;
				inspector->addParameterGroup(this->parameters);

				inspector->addButton("Select channel", [this]() {
					this->spoutReceiver.selectSenderPanel();
					});

				{
					auto preview = ofxCvGui::Panels::makeTexture(this->receiveTexture, "Texture");
					preview->setHeight(300.0f);
					inspector->add(preview);
				}

				inspector->addLiveValue<string>("Channel name", [this]() {
					return this->spoutReceiver.getChannelName();
					});

				inspector->addLiveValue<int>("Width", [this]() {
					return this->spoutReceiver.getWidth();
					});
				inspector->addLiveValue<int>("Height", [this]() {
					return this->spoutReceiver.getHeight();
					});

			}

			//----------
			void
				Spout::render(const RenderSettings& renderSettings)
			{
				if (!this->spoutReceiver.isInitialized()) {
					return;
				}

				if (!this->spoutReceiver.receive(this->receiveTexture)) {
					return;	
				}

				this->receiveTexture.readToPixels(this->receivePixels);

				//Manually resize it since it's 4ch coming in
				{
					auto in = (glm::vec4*) this->receivePixels.getData();
					auto out = (glm::vec3*)this->pixels.getData();

					auto inWidth = this->receivePixels.getWidth();
					auto inHeight = this->receivePixels.getHeight();
					auto outWidth = this->pixels.getWidth();
					auto outHeight = this->pixels.getHeight();

					for (int j = 0; j < outHeight; j++) {
						for (int i = 0; i < outWidth; i++) {
							auto i_in = i * inWidth / outWidth;
							auto j_in = j * inHeight / outHeight;
							out[i + j * outWidth] = in[i_in + j_in * inWidth];
						}
					}
				}
			}
		}
	}
}