#include "pch_App.h"
#include "MotionControl.h"
#include "../Portal.h"
#include "../../Utils.h"

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
			if (this->cachedSentParameters.maxVelocity != this->parameters.maxVelocity.get()
				|| this->cachedSentParameters.acceleration != this->parameters.acceleration.get()
				|| this->cachedSentParameters.minVelocity != this->parameters.minVelocity.get()) {
				this->pushMotionProfile();
			}
		}

		//----------
		void
			MotionControl::populateInspector(ofxCvGui::InspectArguments& args)
		{
			auto inspector = args.inspector;
			inspector->addButton("Push motion profile", [this]() {
				this->pushMotionProfile();
				});
			inspector->addParameterGroup(this->parameters);
		}

		//----------
		void
			MotionControl::move(Steps position)
		{
			auto message = msgpack11::MsgPack::object{
				{
					this->getFWModuleName() , msgpack11::MsgPack::object{
						{"move", position }
					}
				}
			};
			this->portal->sendToPortal(message);
		}

		//----------
		void
			MotionControl::pushMotionProfile()
		{
			auto message = msgpack11::MsgPack::object{
				{
					this->getFWModuleName() , msgpack11::MsgPack::object{
						{
							"motionProfile"
							,  msgpack11::MsgPack::array{
								(int32_t)this->parameters.maxVelocity.get()
								, (int32_t)this->parameters.acceleration.get()
								, (int32_t) this->parameters.minVelocity.get()
							}
						}
					}
				}
			};
			this->portal->sendToPortal(message);

			this->cachedSentParameters.maxVelocity = this->parameters.maxVelocity.get();
			this->cachedSentParameters.acceleration = this->parameters.acceleration.get();
			this->cachedSentParameters.minVelocity = this->parameters.minVelocity.get();
		}
	}
}