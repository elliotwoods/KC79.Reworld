#include "pch_App.h"
#include "../Portal.h"

using namespace msgpack11;

namespace Modules {
	namespace PerPortal {
		//----------
		Axis::Axis(Portal* portal, int axisIndex)
			: portal(portal)
			, axisIndex(axisIndex)
		{
			this->motorDriver = make_shared<MotorDriver>(portal, axisIndex);
			this->motionControl = make_shared<MotionControl>(portal, axisIndex);
		}

		//----------
		string
			Axis::getTypeName() const
		{
			return "Axis";
		}

		//----------
		string
			Axis::getGlyph() const
		{
			return u8"\uf085";
		}

		//----------
		void
			Axis::init()
		{
			this->onPopulateInspector += [this](ofxCvGui::InspectArguments& args) {
				this->populateInspector(args);
			};
		}

		//----------
		void
			Axis::update()
		{
			this->motorDriver->update();
			this->motionControl->update();
		}

		//----------
		void
			Axis::populateInspector(ofxCvGui::InspectArguments& args)
		{
			auto inspector = args.inspector;
			this->motorDriver->addSubMenuToInsecptor(inspector, this->motorDriver);
			this->motionControl->addSubMenuToInsecptor(inspector, this->motionControl);
		}

		//----------
		shared_ptr<MotorDriver>
			Axis::getMotorDriver()
		{
			return this->motorDriver;
		}

		//----------
		shared_ptr<MotionControl>
			Axis::getMotionControl()
		{
			return this->motionControl;
		}

	}
}