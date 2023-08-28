#include "pch_App.h"
#include "Portal.h"

using namespace msgpack11;

namespace Modules {
	//----------
	vector<Portal::Action>
	Portal::getActions()
	{
		return {
			{
				"Initialise routine"
				, u8"\uf11e"
				, msgpack11::MsgPack::object {
					{
						"init", msgpack11::MsgPack()
					}
				}
			}
			, {
				"Calibrate routine"
				, u8"\uf545"
				, msgpack11::MsgPack::object {
					{
						"calibrate", msgpack11::MsgPack()
					}
				}
			}
			, {
				"Flash lights"
				, u8"\uf0eb"
				, msgpack11::MsgPack::object {
					{
						"flashLED", msgpack11::MsgPack()
					}
				}
			}
			, {
				"Home"
				, u8"\uf015"
				, msgpack11::MsgPack::object {
					{
						"m"
						, MsgPack::array {
							0
							, 0
						}
					}
				}
			}
			, {
				"See through"
				, u8"\uf06e"
				, msgpack11::MsgPack::object {
					{
						"m"
						, MsgPack::array {
							MOTION_MICROSTEPS_PER_PRISM_ROTATION / 2
							, 0
						}
					}
				}
			}
			, {
				"Unjam"
				, u8"\uf6e3"
				, msgpack11::MsgPack::object {
					{
						"unjam", msgpack11::MsgPack()
					}
				}
			}
			, {
				"Escape from routine"
				, u8"\uf2f5"
				, msgpack11::MsgPack::object {
					{
						"escapeFromRoutine", msgpack11::MsgPack()
					}
				}
			}
			, {
				"Reboot"
				, u8"\uf011"
				, msgpack11::MsgPack::object {
					{
						"reset", msgpack11::MsgPack()
					}
				}
			}
		};
	}

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
		Portal::populateInspectorPanelHeader(ofxCvGui::InspectArguments& args)
	{
		auto inspector = args.inspector;

		auto buttonStack = inspector->addHorizontalStack();

		// Add title number for portal ID
		buttonStack->add(make_shared<ofxCvGui::Widgets::Title>("#" + ofToString((int) this->getTarget())));

		// Add actions
		auto actions = Portal::getActions();

		// Special action for poll
		buttonStack->addButton("Poll", [this]() {
			this->poll();
			}, ' ')->setDrawGlyph(u8"\uf059");


		for (const auto& action : actions) {
			auto hasHotkey = action.shortcutKey != 0;
			auto buttonAction = [this, action]() {
				this->sendToPortal(action.message);
			};

			auto button = hasHotkey
				? buttonStack->addButton(action.caption, buttonAction, action.shortcutKey)
				: buttonStack->addButton(action.caption, buttonAction);

			button->setDrawGlyph(action.icon);
		}
	}

	//----------
	void
		Portal::populateInspector(ofxCvGui::InspectArguments& args)
	{
		auto inspector = args.inspector;

		this->populateInspectorPanelHeader(args);

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