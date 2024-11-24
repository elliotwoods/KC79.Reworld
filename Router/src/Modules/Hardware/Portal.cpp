#include "pch_App.h"
#include "Portal.h"
#include "../App.h"

using namespace msgpack11;

namespace Modules {
	//----------
	vector<Portal::Action>
	Portal::getActions()
	{
		return {
			{
				"Ping"
				, u8"\uf45d"
				, msgpack11::MsgPack()
				, "/ping"
			}
			, {
				"Initialise routine"
				, u8"\uf11e"
				, msgpack11::MsgPack::object {
					{
						"init", msgpack11::MsgPack()
					}
				}
				, "/init"
			,
			}
			, {
				"Calibrate routine"
				, u8"\uf545"
				, msgpack11::MsgPack::object {
					{
						"calibrate", msgpack11::MsgPack()
					}
				}
				, "/calibrate"
			}
			, {
				"Home routine"
				, u8"\uf3fd"
				, msgpack11::MsgPack::object {
					{
						"home", msgpack11::MsgPack()
					}
				}
				, "/home"
			}
			, {
				"Flash lights"
				, u8"\uf0eb"
				, msgpack11::MsgPack::object {
					{
						"flashLED", msgpack11::MsgPack()
					}
				}
				, "/flashLEDs"
			}
			, {
				"Go Home"
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
				, "/goHome"
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
				, "/seeThrough"
			}
			, {
				"Disable motor lights"
				, u8"\uf042"
				, msgpack11::MsgPack::object {
					{
						"motorIndicatorEnabled", false
					}
				}
				, "/disableMotorLights"
			}
			, {
				"Enable motor lights"
				, u8"\uf111"
				, msgpack11::MsgPack::object {
					{
						"motorIndicatorEnabled", true
					}
				}
				, "/enableMotorLights"
			}
			, {
				"Unjam"
				, u8"\uf6e3"
				, msgpack11::MsgPack::object {
					{
						"unjam", msgpack11::MsgPack()
					}
				}
				, "/unjam"
			}
			, {
				"Escape from routine"
				, u8"\uf2f5"
				, msgpack11::MsgPack::object {
					{
						"escapeFromRoutine", msgpack11::MsgPack()
					}
				}
				, "/escapeFromRoutine"
			}
			, {
				"Reboot"
				, u8"\uf011"
				, msgpack11::MsgPack::object {
					{
						"reset", msgpack11::MsgPack()
					}
				}
				, "/reboot"
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
	shared_ptr<ofxCvGui::Widgets::Button>
		Portal::makeButton(shared_ptr<Modules::Portal> portal)
	{
		auto action = [portal]() {
			ofxCvGui::inspect(portal);
		};
		auto numberString = ofToString((int)portal->getTarget());

		// With or without hotkey
		auto button = (int)portal->getTarget() < 10
			? make_shared<ofxCvGui::Widgets::Button>(numberString, action, numberString[0])
			: make_shared<ofxCvGui::Widgets::Button>(numberString, action);

		// Add sub-widgets
		{
			button->addChild(portal->storedWidgets.rxHeartbeat);
			button->addChild(portal->storedWidgets.txHeartbeat);
			button->addChild(portal->storedWidgets.position);

			button->onBoundsChange += [portal](ofxCvGui::BoundsChangeArguments& args) {
				auto width = args.localBounds.width / 3.0f;
				portal->storedWidgets.rxHeartbeat->setWidth(width);
				portal->storedWidgets.rxHeartbeat->setPosition({ 0, 0 });
				portal->storedWidgets.txHeartbeat->setWidth(width);
				portal->storedWidgets.txHeartbeat->setPosition({ 0, args.localBounds.height / 2.0f });
				portal->storedWidgets.position->setBounds(ofRectangle{ width * 2, 0, width, args.localBounds.height });
			};
		}

		return button;
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

		{
			this->storedWidgets.rxHeartbeat = make_shared<ofxCvGui::Widgets::Heartbeat>("Rx", [this]() {
				return this->isFrameNew.rx.isFrameNew;
				});
			this->storedWidgets.txHeartbeat = make_shared<ofxCvGui::Widgets::Heartbeat>("Tx", [this]() {
				return this->isFrameNew.tx.isFrameNew;
				});

			this->storedWidgets.position = ofxCvGui::makeElement();
			{
				this->storedWidgets.position->onDraw += [this](ofxCvGui::DrawArguments& args) {

					auto r = min(args.localBounds.width, args.localBounds.height) / 2.0f;

					ofPushMatrix();
					{
						ofTranslate(args.localBounds.getCenter());

						ofPushStyle();
						{
							// grid
							ofNoFill();
							ofSetColor(100);
							ofDrawCircle(0, 0, r);
							ofDrawLine(-r, 0, r, 0);
							ofDrawLine(0, -r, 0, r);

							ofFill();

							// live position
							{
								ofSetColor(100, 100, 200);
								auto position = this->getPilot()->getLivePosition();
								ofDrawCircle({
									position.x * r
									, position.y * r
									}, 2.0f);
							}

							// target position (local)
							{
								ofSetColor(255);
								auto position = this->getPilot()->getPosition();
								ofDrawCircle({
									position.x * r
									, position.y * r
									}, 2.0f);
							}
						}
						ofPopStyle();
					}
					ofPopMatrix();
				};
			}
		}
	}

	//----------
	void
		Portal::update()
	{
		for (auto submodule : this->submodules) {
			submodule->update();
		}

		{
			this->isFrameNew.rx.update();
			this->isFrameNew.tx.update();
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

		// Horizontal stack for status
		{
			auto stack = inspector->addHorizontalStack();

			{

				// Add title number for portal ID
				stack->add(make_shared<ofxCvGui::Widgets::Title>("#" + ofToString((int)this->getTarget())));

				// Time since last message
				stack->add(make_shared<ofxCvGui::Widgets::LiveValueHistory>("Time since last message", [this]() {
					return (float)chrono::duration_cast<chrono::milliseconds>(chrono::system_clock::now() - this->lastIncoming).count() / 1000.0f;
					}));

				// Heartbeats
				stack->add(this->storedWidgets.rxHeartbeat);
				stack->add(this->storedWidgets.txHeartbeat);
				stack->add(Utils::makeGUIElement(&this->reportedState.upTime));

				// Last log message
				stack->add(make_shared<ofxCvGui::Widgets::LiveValue<string>>("Last log message", [this]() {
					auto message = this->logger->getLatestMessage();
					if (!message) {
						return string("");
					}
					else {
						return message->message;
					}
					}));
			}
		}

		// Button stack of actions
		{
			auto buttonStack = inspector->addHorizontalStack();

			// Add actions
			auto actions = Portal::getActions();

			// Special action for poll
			buttonStack->addButton("Poll", [this]() {
				this->poll();
				}, ' ')->setDrawGlyph(u8"\uf059");


			for (const auto& action : actions) {
				if (buttonStack->getElements().size() >= 6) {
					buttonStack = inspector->addHorizontalStack();
				}

				auto hasHotkey = action.shortcutKey != 0;
				auto buttonAction = [this, action]() {
					this->sendToPortal(action.message, "");
				};

				auto button = hasHotkey
					? buttonStack->addButton(action.caption, buttonAction, action.shortcutKey)
					: buttonStack->addButton(action.caption, buttonAction);

				button->setDrawGlyph(action.icon);
			}
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
		this->isFrameNew.rx.notify();

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
		Portal::ping()
	{
		this->sendToPortal(msgpack11::MsgPack(), "");
	}

	//----------
	void
		Portal::poll()
	{
		this->sendToPortal(msgpack11::MsgPack::object{
				{
					"poll", msgpack11::MsgPack()
				}
			}, "poll");
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
	bool
		Portal::isRS485Open() const
	{
		return this->rs485->isConnected();
	}

	//----------
	void
		Portal::sendToPortal(const msgpack11::MsgPack& message, const string& address)
	{
		// [target, source, message]
		auto packet = RS485::Packet(
			msgpack11::MsgPack::array{
				(int8_t)this->parameters.targetID.get()
				, (int8_t)0
				, message
			}
		);

		// Info for collate
		packet.target = this->parameters.targetID.get();
		packet.address = address;

		packet.onSent = [this]() {
			this->isFrameNew.tx.notify();
		};


		this->rs485->transmit(packet);
	}

	//----------
	void
		Portal::sendToPortal(const function<msgpack11::MsgPack()>& lazyMessageRenderer, const string& address)
	{
		// [target, source, message]
		auto packet = RS485::Packet([lazyMessageRenderer, this]() {
			return msgpack11::MsgPack::array{
				(int8_t)this->parameters.targetID.get()
				, (int8_t)0
				, lazyMessageRenderer()
			};
		});

		// Info for collate
		packet.target = this->parameters.targetID.get();
		packet.address = address;

		packet.onSent = [this]() {
			this->isFrameNew.tx.notify();
		};

		this->rs485->transmit(packet);
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

	//----------
	vector<ofxCvGui::ElementPtr>
		Portal::getWidgets()
	{
		return {
			this->storedWidgets.rxHeartbeat
			, this->storedWidgets.txHeartbeat
		};
	}

}