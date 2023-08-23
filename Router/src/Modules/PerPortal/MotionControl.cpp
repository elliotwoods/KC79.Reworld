#include "pch_App.h"
#include "MotionControl.h"
#include "../Portal.h"
#include "../../Utils.h"

using namespace msgpack11;

namespace Modules {
	namespace PerPortal {
		//----------
		MotionControl::MotionControl(Portal* portal, int axisIndex)
			: portal(portal)
			, axisIndex(axisIndex)
		{
		}

		//----------
		string
			MotionControl::getTypeName() const
		{
			return "MotionControl";
		}

		//----------
		string
			MotionControl::getGlyph() const
		{
			return u8"\uf2f1";
		}

		//----------
		string
			MotionControl::getName() const
		{
			return "MotionControl" + Utils::getAxisLetter(this->axisIndex);
		}

		//----------
		string
			MotionControl::getFWModuleName() const
		{
			return "motionControl" + Utils::getAxisLetter(this->axisIndex);
		}

		//----------
		void
			MotionControl::init()
		{
			this->onPopulateInspector += [this](ofxCvGui::InspectArguments& args) {
				this->populateInspector(args);
			};
		}

		//----------
		void
			MotionControl::update()
		{
			if (this->cachedSentParameters.maxVelocity != this->parameters.motionProfile.maxVelocity.get()
				|| this->cachedSentParameters.acceleration != this->parameters.motionProfile.acceleration.get()
				|| this->cachedSentParameters.minVelocity != this->parameters.motionProfile.minVelocity.get()) {
				this->pushMotionProfile();
			}
		}

		//----------
		void
			MotionControl::populateInspector(ofxCvGui::InspectArguments& args)
		{
			auto inspector = args.inspector;

			auto buttonStack = inspector->addHorizontalStack();
			{
				{
					auto button = make_shared<ofxCvGui::Widgets::Button>("zeroCurrentPosition", [this]() {
						this->zeroCurrentPosition();
						});
					button->setDrawGlyph(u8"\uf192");
					buttonStack->add(button);
				}
				{
					auto button = make_shared<ofxCvGui::Widgets::Button>("measureBacklash", [this]() {
						this->measureBacklash();
						});
					button->setDrawGlyph(u8"\uf545");
					buttonStack->add(button);
				}
				{
					auto button = make_shared<ofxCvGui::Widgets::Button>("homeRoutine", [this]() {
						this->homeRoutine();
						});
					button->setDrawGlyph(u8"\uf015");
					buttonStack->add(button);
				}
				{
					auto button = make_shared<ofxCvGui::Widgets::Button>("deinitTimer", [this]() {
						this->deinitTimer();
						});
					button->setDrawGlyph(u8"\uf28d");
					buttonStack->add(button);
				}
				{
					auto button = make_shared<ofxCvGui::Widgets::Button>("initTimer", [this]() {
						this->initTimer();
						});
					button->setDrawGlyph(u8"\uf144");
					buttonStack->add(button);
				}
				{
					auto button = make_shared<ofxCvGui::Widgets::Button>("testTimer", [this]() {
						this->testTimer();
						});
					button->setDrawGlyph(u8"\uf492");
					buttonStack->add(button);
				}
				{
					auto button = make_shared<ofxCvGui::Widgets::Button>("Push motion profile", [this]() {
						this->pushMotionProfile();
						});
					button->setDrawGlyph(u8"\uf093");
					buttonStack->add(button);
				}
			}

			for (const auto& variable : this->reportedState.variables) {
				inspector->add(Utils::makeGUIElement(variable));
			}

			inspector->addParameterGroup(this->parameters);
		}

		//----------
		void
			MotionControl::processIncoming(const nlohmann::json& json)
		{
			for (const auto & variable : this->reportedState.variables) {
				variable->processIncoming(json);
			}
		}

		//----------
		void
			MotionControl::move(Steps targetPosition)
		{
			auto message = msgpack11::MsgPack::object{
				{
					this->getFWModuleName() , msgpack11::MsgPack::object{
						{"move", targetPosition }
					}
				}
			};
			this->portal->sendToPortal(message);
		}

		//----------
		void
			MotionControl::move(Steps targetPosition
				, StepsPerSecond maxVelocity
				, StepsPerSecondPerSecond acceleration
				, StepsPerSecond minVelocity)
		{
			auto message = MsgPack::object{
				{
					this->getFWModuleName() , MsgPack::object{
						{
							"move"
							, MsgPack::array{
								targetPosition
								, maxVelocity
								, acceleration
								, minVelocity
							}
						}
					}
				}
			};
			this->portal->sendToPortal(message);
		}

		//----------
		void
			MotionControl::zeroCurrentPosition()
		{
			auto message = MsgPack::object{
				{
					this->getFWModuleName() , MsgPack::object{
						{
							"zeroCurrentPosition"
							, msgpack11::MsgPack()
						}
					}
				}
			};
			this->portal->sendToPortal(message);
		}

		//----------
		MsgPack
			MotionControl::getMeasureSettings() const
		{
			return MsgPack::array{
				(uint8_t) this->parameters.measureSettings.timeout_s
				, (int32_t)this->parameters.measureSettings.slowSpeed
				, (int32_t)this->parameters.measureSettings.backOffDistance
				, (int32_t)this->parameters.measureSettings.debounceDistance
			};
		}

		//----------
		void
			MotionControl::measureBacklash()
		{
			auto message = MsgPack::object{
				{
					this->getFWModuleName() , MsgPack::object{
						{
							"measureBacklash"
							, this->getMeasureSettings()
						}
					}
				}
			};
			this->portal->sendToPortal(message);
		}

		//----------
		void
			MotionControl::homeRoutine()
		{
			auto message = MsgPack::object{
				{
					this->getFWModuleName() , MsgPack::object{
						{
							"home"
							, this->getMeasureSettings()
						}
					}
				}
			};
			this->portal->sendToPortal(message);
		}

		//----------
		bool
			MotionControl::getCurrentPositionKnown() const
		{
			return this->reportedState.position.hasBeenReported;
		}

		//----------
		Steps
			MotionControl::getCurrentPosition() const
		{
			return this->reportedState.position.value;
		}

		//----------
		bool
			MotionControl::getTargetPositionKnown() const
		{
			return this->reportedState.targetPosition.hasBeenReported;
		}

		//----------
		Steps
			MotionControl::getTargetPosition() const
		{
			return this->reportedState.targetPosition.value;
		}

		//----------
		void
			MotionControl::setReportedCurrentPosition(Steps value)
		{
			this->reportedState.position.value = value;
			this->reportedState.position.hasBeenReported = true;

		}

		//----------
		void
			MotionControl::setReportedTargetPosition(Steps value)
		{
			this->reportedState.targetPosition.value = value;
			this->reportedState.targetPosition.hasBeenReported = true;
		}

		//----------
		void
			MotionControl::testTimer()
		{
			auto message = MsgPack::object{
				{
					this->getFWModuleName() , MsgPack::object{
						{
							"testTimer"
							, MsgPack()
						}
					}
				}
			};
			this->portal->sendToPortal(message);
		}

		//----------
		void
			MotionControl::deinitTimer()
		{
			auto message = MsgPack::object{
				{
					this->getFWModuleName() , MsgPack::object{
						{
							"deinitTimer"
							, MsgPack()
						}
					}
				}
			};
			this->portal->sendToPortal(message);
		}

		//----------
		void
			MotionControl::initTimer()
		{
			auto message = MsgPack::object{
				{
					this->getFWModuleName() , MsgPack::object{
						{
							"initTimer"
							, MsgPack()
						}
					}
				}
			};
			this->portal->sendToPortal(message);
		}

		//----------
		void
			MotionControl::pushMotionProfile()
		{
			auto message = MsgPack::object{
				{
					this->getFWModuleName() , MsgPack::object{
						{
							"motionProfile"
							,  MsgPack::array{
								(int32_t)this->parameters.motionProfile.maxVelocity.get()
								, (int32_t)this->parameters.motionProfile.acceleration.get()
								, (int32_t) this->parameters.motionProfile.minVelocity.get()
							}
						}
					}
				}
			};
			this->portal->sendToPortal(message);

			this->cachedSentParameters.maxVelocity = this->parameters.motionProfile.maxVelocity.get();
			this->cachedSentParameters.acceleration = this->parameters.motionProfile.acceleration.get();
			this->cachedSentParameters.minVelocity = this->parameters.motionProfile.minVelocity.get();
		}
	}
}