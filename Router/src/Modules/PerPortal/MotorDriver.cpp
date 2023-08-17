#include "pch_App.h"
#include "MotorDriver.h"
#include "../Portal.h"

using namespace msgpack11;

namespace Modules {
	namespace PerPortal {
		//----------
		MotorDriver::MotorDriver(Portal* portal, int axisIndex)
			: portal(portal)
			, axisIndex(axisIndex)
		{

		}

		//----------
		string
			MotorDriver::getTypeName() const
		{
			return "MotorDriver";
		}

		//----------
		string
			MotorDriver::getGlyph() const
		{
			return u8"\uf013";
		}


		//----------
		void
			MotorDriver::init()
		{
			this->onPopulateInspector += [this](ofxCvGui::InspectArguments& args) {
				this->populateInspector(args);
			};
		}

		//----------
		void
			MotorDriver::update()
		{

		}

		//----------
		void
			MotorDriver::populateInspector(ofxCvGui::InspectArguments& args)
		{
			auto inspector = args.inspector;
		}
	}
}