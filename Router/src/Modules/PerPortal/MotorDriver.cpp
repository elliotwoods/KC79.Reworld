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

			inspector->addButton("testRoutine", [this]() {
				this->testRoutine();
				});
			inspector->addButton("testTimer", [this]() {
				this->testTimer();
				});
			inspector->addParameterGroup(this->parameters);
		}

		//----------
		void
			MotorDriver::testRoutine()
		{
			auto message = MsgPack::object{
				{
					"motorDriver" + Utils::getAxisLetter(this->axisIndex) , MsgPack::object{
						{
							"testRoutine", MsgPack()
						}
					}
				}
			};
			this->portal->sendToPortal(message);
		}

		//----------
		void
			MotorDriver::testTimer()
		{
			auto period = this->parameters.testTimer.period.get();
			auto count = this->parameters.testTimer.count.get();

			if (this->parameters.testTimer.normaliseParameters.get()) {
				auto microstepResolution = this->portal->getMotorDriverSettings()->getMicrostep();
				period /= microstepResolution;
				count *= microstepResolution;
			}
			auto message = MsgPack::object{
				{
					"motorDriver" + Utils::getAxisLetter(this->axisIndex) , MsgPack::object{
						{
							"testTimer", MsgPack::array{
								(uint32_t)this->parameters.testTimer.period.get()
								, (uint32_t) this->parameters.testTimer.count.get()
							}

						}
					}
				}
			};
			this->portal->sendToPortal(message);
		}
	}
}