#include "pch_App.h"
#include "Portal.h"

namespace Modules {
	//----------
	Portal::Portal(shared_ptr<RS485> rs485, int targetID)
		: rs485(rs485)
	{
		this->parameters.targetID = targetID;
	}

	//----------
	string
		Portal::getTypeName() const
	{
		return "Portal";
	}

	//----------
	string
		Portal::getGlyph() const
	{
		return u8"\uf111";
	}

	//----------
	void
		Portal::init()
	{
		this->onPopulateInspector += [this](ofxCvGui::InspectArguments& args) {
			this->populateInspector(args);
		};

		this->motorDriverSettings = make_shared<PerPortal::MotorDriverSettings>(this);
		this->axis[0] = make_shared<PerPortal::Axis>(this, 0);
		this->axis[1] = make_shared<PerPortal::Axis>(this, 1);
		this->pilot = make_shared<PerPortal::Pilot>(this);
		this->submodules = {
			this->motorDriverSettings
			, this->axis[0]
			, this->axis[1]
			, this->pilot
		};

		for (auto submodule : submodules) {
			submodule->init();
		}
	}

	//----------
	void
		Portal::update()
	{
		for (auto submodule : this->submodules) {
			submodule->update();
		}
	}

	//----------
	void
		Portal::populateInspector(ofxCvGui::InspectArguments& args)
	{
		auto inspector = args.inspector;

		for (auto submodule : this->submodules) {
			submodule->addSubMenuToInsecptor(inspector, submodule);
		}
	}

	//----------
	void
		Portal::sendToPortal(const msgpack11::MsgPack& message)
	{
		// Format as array [targetID, sourceID, message]
		this->rs485->transmit(msgpack11::MsgPack::array{
			(int8_t)this->parameters.targetID.get()
			, (int8_t) 0
			, message
			});
	}

	//----------
	shared_ptr<PerPortal::Axis>
		Portal::getAxis(int axisIndex)
	{
		return this->axis[axisIndex];
	}
}