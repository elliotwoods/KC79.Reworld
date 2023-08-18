#include "pch_App.h"
#include "../Portal.h"

using namespace msgpack11;

namespace Modules {
	namespace PerPortal {
		//----------
		MotorDriverSettings::MotorDriverSettings(Portal* portal)
			: portal(portal)
		{

		}

		//----------
		string
			MotorDriverSettings::getTypeName() const
		{
			return "MotorDriverSettings";
		}

		//----------
		string
			MotorDriverSettings::getGlyph() const
		{
			return u8"\uf1de";
		}

		//----------
		void
			MotorDriverSettings::init()
		{
			this->onPopulateInspector += [this](ofxCvGui::InspectArguments& args) {
				this->populateInspector(args);
			};
		}

		//----------
		void
			MotorDriverSettings::update()
		{
			if (this->parameters.autoPush) {
				if (this->cachedSentValues.current != this->parameters.current) {
					this->pushCurrent();
				}
				if(this->cachedSentValues.microstepResolution != this->parameters.microstepResolution) {
					this->pushMicrostepResolution();
				}
			}
		}

		//----------
		void
			MotorDriverSettings::populateInspector(ofxCvGui::InspectArguments& args)
		{
			auto inspector = args.inspector;
			inspector->addParameterGroup(this->parameters);
			inspector->addButton("Push values", [this]() {
				this->pushValues();
				});
		}

		//----------
		void
			MotorDriverSettings::pushValues()
		{
			this->pushCurrent();
			this->pushMicrostepResolution();
		}

		//---------
		void
			MotorDriverSettings::pushCurrent()
		{
			auto value = this->parameters.current.get();
			auto message = MsgPack::object{
				{
					"motorDriverSettings", MsgPack::object {
						{"setCurrent", (float)value }
					}
				}
			};
			this->portal->sendToPortal(message);
			this->cachedSentValues.current = value;

		}

		//---------
		void
			MotorDriverSettings::pushMicrostepResolution()
		{
			auto value = this->parameters.microstepResolution.get();
			auto valueFormatted = (uint8_t) log2(value);
			auto message = MsgPack::object{
				{
					"motorDriverSettings", MsgPack::object {
						{"setMicrostepResolution", (uint8_t)valueFormatted }
					}
				}
			};
			this->portal->sendToPortal(message);
			this->cachedSentValues.microstepResolution = this->parameters.microstepResolution.get();
		}

		//---------
		Steps
			MotorDriverSettings::getMicrostep() const
		{
			return this->parameters.microstepResolution.get();
		}
	}
}