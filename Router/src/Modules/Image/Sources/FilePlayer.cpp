#include "pch_App.h"
#include "FilePlayer.h"

namespace Modules {
	namespace Image {
		namespace Sources {
			//----------
			FilePlayer::FilePlayer()
			{

			}

			//----------
			string
				FilePlayer::getTypeName() const
			{
				return "Image::Sources::FilePlayer";
			}

			//----------
			void
				FilePlayer::init()
			{
				this->onPopulateInspector += [this](ofxCvGui::InspectArguments& args) {
					this->populateInspector(args);
					};

				// Add a listener to the position parameter to update the player position
				this->parameters.position.addListener(this, &FilePlayer::onPositionJumped);
			}

			//----------
			void
				FilePlayer::update()
			{
				// Load the file if it's changed
				{
					auto file = this->parameters.file.get();
					
					// Selected a different file / selection is cleared
					if(this->player.getMoviePath() != file.string()) {
						this->player.close();
					}
					
					// Something is selected
					if (!file.empty() && !this->player.isLoaded()) {
						this->player.load(file.string());
						this->player.play();

						switch (this->parameters.loopMode.get()) {
						case LoopMode::Loop:
							this->player.setLoopState(OF_LOOP_NORMAL);
							break;
						case LoopMode::PingPong:
							this->player.setLoopState(OF_LOOP_PALINDROME);
							break;
						case LoopMode::None:
							this->player.setLoopState(OF_LOOP_NONE);
							break;
						}
						this->player.setSpeed(this->parameters.speed.get());
					}
				}
				
				// Do the playing
				if (this->player.isLoaded()) {
					this->player.setPaused(!this->parameters.play.get());
					this->player.setSpeed(this->parameters.speed.get());

					if (this->parameters.play.get()) {
						this->player.update();
						this->disableJump = true;
						this->parameters.position.set(this->player.getPosition());
						this->disableJump = false;
					}
				}
			}

			//----------
			void
				FilePlayer::populateInspector(ofxCvGui::InspectArguments& args)
			{
				auto inspector = args.inspector;
				inspector->addParameterGroup(this->parameters);
				inspector->addIndicatorBool("Loaded", [this]() {
					return this->player.isLoaded();
					});
				inspector->addLiveValue<float>("Duration", [this]() {
					return this->player.getDuration();
					});
				inspector->addButton("Clear", [this]() {
					this->parameters.file.set("");
					});
				inspector->addButton("Jump to start", [this]() {
					this->player.setPosition(0.0f);
					});
			}

			//----------
			void
				FilePlayer::render(const RenderSettings& renderSettings)
			{
				if (this->player.isLoaded()) {
					// resample the pixels from the player into the pixels object
					auto playerPixels = this->player.getPixels();
					if (playerPixels.isAllocated()) {
						// create an 8bit buffer of the correct size to copy into
						this->pixels8.allocate(renderSettings.width, renderSettings.height, 3);

						// copy into the 8bit buffer
						playerPixels.resizeTo(this->pixels8);

						// copy into the float buffer
						for (auto i = 0; i < this->pixels8.size(); i++) {
							this->pixels[i] = (float) this->pixels8[i] / 255.0f;
						}
					}
					this->player.draw(0, 0, renderSettings.width, renderSettings.height);
				}
			}

			//----------
			void
				FilePlayer::onPositionJumped(float& position)
			{
				if (this->player.isLoaded() && !this->disableJump) {
					this->player.setPosition(position);
				}
			}
		}
	}
}