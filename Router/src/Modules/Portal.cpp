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
		this->logger = make_shared<PerPortal::Logger>(this);

		this->submodules = {
			this->motorDriverSettings
			, this->axis[0]
			, this->axis[1]
			, this->pilot
			, logger
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

		if (this->parameters.poll.regularly) {
			if (chrono::system_clock::now() - lastPoll > chrono::milliseconds((int) (this->parameters.poll.interval.get() * 1000.0f))) {
				this->poll();
			}
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

		inspector->addSpacer();

		inspector->addLiveValueHistory("Time since last message", [this]() {
			return (float) chrono::duration_cast<chrono::milliseconds>(chrono::system_clock::now() - this->lastIncoming).count() / 1000.0f;
			});

		inspector->addButton("Poll", [this]() {
			this->poll();
			});


		inspector->addButton("initRoutine", [this]() {
			this->initRoutine();
			});

		inspector->addParameterGroup(this->parameters);
	}

	//----------
	void
		Portal::processIncoming(const nlohmann::json& json)
	{
		this->lastIncoming = chrono::system_clock::now();

		if (json.contains("mca")) {
			this->axis[0]->getMotionControl()->processIncoming(json["mca"]);
		}
		if (json.contains("mcb")) {
			this->axis[1]->getMotionControl()->processIncoming(json["mcb"]);
		}
		if (json.contains("logger")) {
			this->logger->processIncoming(json["logger"]);
		}
	}

	//----------
	void
		Portal::poll()
	{
		this->sendToPortal(msgpack11::MsgPack::object{
				{
					"poll", msgpack11::MsgPack()
				}
			});
		this->lastPoll = chrono::system_clock::now();
	}

	//----------
	void
		Portal::initRoutine()
	{
		this->sendToPortal(msgpack11::MsgPack::object{
				{
					"init", msgpack11::MsgPack()
				}
			});
		this->lastPoll = chrono::system_clock::now();
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
	shared_ptr<PerPortal::MotorDriverSettings>
		Portal::getMotorDriverSettings()
	{
		return this->motorDriverSettings;
	}

	//----------
	shared_ptr<PerPortal::Axis>
		Portal::getAxis(int axisIndex)
	{
		return this->axis[axisIndex];
	}
}