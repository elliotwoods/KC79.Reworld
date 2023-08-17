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

		}

		//----------
		void
			MotionControl::populateInspector(ofxCvGui::InspectArguments& args)
		{
			auto inspector = args.inspector;
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
	}
}