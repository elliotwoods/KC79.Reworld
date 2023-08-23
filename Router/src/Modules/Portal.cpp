#include "pch_App.h"
#include "Portal.h"

namespace Modules {
	//----------
	Portal::Portal(shared_ptr<RS485> rs485, int targetID)
		: rs485(rs485)
	{
		this->parameters.targetID = targetID;
		this->init();
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

		inspector->addLiveValue<string>("Last log message", [this]() {
			auto message = this->logger->getLatestMessage();
			if (!message) {
				return string("");
			}
			else {
				return message->message;
			}
			});

		auto buttonStack = inspector->addHorizontalStack();
		{
			buttonStack->addButton("Poll", [this]() {
				this->poll();
			}, ' ')->setDrawGlyph(u8"\uf059");

			buttonStack->addButton("Initialise routine", [this]() {
				this->initRoutine();
			})->setDrawGlyph(u8"\uf11e");

			buttonStack->addButton("Calibrate routine", [this]() {
				this->calibrateRoutine();
				})->setDrawGlyph(u8"\uf545");

			buttonStack->addButton("Flash lights", [this]() {
				this->flashLEDsRoutine();
			})->setDrawGlyph(u8"\uf0eb");

			buttonStack->addButton("Reset", [this]() {
				this->reset();
				})->setDrawGlyph(u8"\uf011");
		}

		for (auto variable : this->reportedState.variables) {
			inspector->add(Utils::makeGUIElement(variable));
		}

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
		if (json.contains("app")) {
			for (const auto& variable : this->reportedState.variables) {
				variable->processIncoming(json["app"]);
			}
		}
		if (json.contains("p")) {
			// it's the succinct position report
			auto motionControlA = this->getAxis(0)->getMotionControl();
			auto motionControlB = this->getAxis(1)->getMotionControl();

			if (json["p"].size() >= 1) {
				motionControlA->setReportedCurrentPosition(json["p"][0]);
			}
			if (json["p"].size() >= 2) {
				motionControlB->setReportedCurrentPosition(json["p"][1]);
			}
			if (json["p"].size() >= 3) {
				motionControlA->setReportedTargetPosition(json["p"][2]);
			}
			if (json["p"].size() >= 4) {
				motionControlB->setReportedTargetPosition(json["p"][3]);
			}
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
		Portal::calibrateRoutine()
	{
		this->sendToPortal(msgpack11::MsgPack::object{
				{
					"calibrate", msgpack11::MsgPack()
				}
			});
		this->lastPoll = chrono::system_clock::now();
	}

	//----------
	void
		Portal::flashLEDsRoutine()
	{
		this->sendToPortal(msgpack11::MsgPack::object{
				{
					"flashLED", msgpack11::MsgPack::array {
						(uint16_t)this->parameters.flash.period.get()
						, (uint16_t)this->parameters.flash.count.get()
					}
				}
			});
		this->lastPoll = chrono::system_clock::now();
	}

	//----------
	void
		Portal::reset()
	{
		this->sendToPortal(msgpack11::MsgPack::object{
				{
					"reset", msgpack11::MsgPack()
				}
			});
	}

	//----------
	Portal::Target
		Portal::getTarget() const
	{
		return this->parameters.targetID.get();
	}

	//----------
	void
		Portal::setTarget(Target value)
	{
		this->parameters.targetID.set(value);
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

	//----------
	shared_ptr<PerPortal::Pilot>
		Portal::getPilot()
	{
		return this->pilot;
	}
}