#include "pch_App.h"
#include "ModuleControl.h"

namespace Modules {
	//---------
	ModuleControl::ModuleControl(shared_ptr<RS485> rs485)
		: rs485(rs485)
	{

	}

	//---------
	string
		ModuleControl::getTypeName() const
	{
		return "ModuleControl";
	}

	//---------
	void
		ModuleControl::init()
	{
		this->onPopulateInspector += [this](ofxCvGui::InspectArguments& args) {
			this->populateInspector(args);
		};
	}

	//---------
	void
		ModuleControl::update()
	{
		if (this->debug.cachedValues.motorDriverSettings.current != this->parameters.debug.current) {
			this->setCurrent(this->parameters.debug.targetID.get(), this->parameters.debug.current.get());
		}
	}

	//---------
	void
		ModuleControl::populateInspector(ofxCvGui::InspectArguments& args)
	{
		auto inspector = args.inspector;

		inspector->addParameterGroup(this->parameters);

		inspector->addButton("testRoutine A", [this]() {
			this->runTestRoutine(this->parameters.debug.targetID.get(), Axis::A);
			});

		inspector->addButton("testRoutine B", [this]() {
			this->runTestRoutine(this->parameters.debug.targetID.get(), Axis::B);
			});
	}

	//---------
	void
		ModuleControl::setCurrent(RS485::Target target, float value)
	{
		auto rs485 = this->rs485.lock();
		if (!rs485) {
			ofLogError("No RS485");
			return;
		}

		msgpack_sbuffer messageBuffer;
		msgpack_packer packer;
		msgpack_sbuffer_init(&messageBuffer);
		msgpack_packer_init(&packer
			, &messageBuffer
			, msgpack_sbuffer_write);

		{
			msgpack_pack_map(&packer, 1);

			// (0) - Key
			{
				string key = "motorDriverSettings";
				msgpack_pack_str(&packer, key.size());
				msgpack_pack_str_body(&packer, key.c_str(), key.size());
			}

			// (0) - Value
			{
				msgpack_pack_map(&packer, 1);

				// (1) - Key
				{
					string key = "setCurrent";
					msgpack_pack_str(&packer, key.size());
					msgpack_pack_str_body(&packer, key.c_str(), key.size());
				}

				// (1) - Value
				{
					msgpack_pack_float(&packer, value);
				}
			}
		}

		auto header = rs485->makeHeader(target);

		vector<uint8_t> body;
		body.assign((uint8_t*)(messageBuffer.data)
			, (uint8_t*)(messageBuffer.data + messageBuffer.size));

		rs485->transmitHeaderAndBody(header, body);
		msgpack_sbuffer_destroy(&messageBuffer);

		this->debug.cachedValues.motorDriverSettings.current = value;
	}

	//---------
	void
		ModuleControl::runTestRoutine(RS485::Target target, Axis axis)
	{
		auto rs485 = this->rs485.lock();
		if (!rs485) {
			ofLogError("No RS485");
			return;
		}
		
		msgpack_sbuffer messageBuffer;
		msgpack_packer packer;
		msgpack_sbuffer_init(&messageBuffer);
		msgpack_packer_init(&packer
			, &messageBuffer
			, msgpack_sbuffer_write);

		{
			msgpack_pack_map(&packer, 1);

			// (0) - Key
			{
				string key;
				switch (axis) {
				case Axis::A:
					key = "motorDriverA";
					break;
				case Axis::B:
					key = "motorDriverB";
					break;
				default:
					break;
				}

				msgpack_pack_str(&packer, key.size());
				msgpack_pack_str_body(&packer, key.c_str(), key.size());
			}

			// (0) - Value
			{
				msgpack_pack_map(&packer, 1);

				// (1) - Key
				{
					string key = "testRoutine";

					msgpack_pack_str(&packer, key.size());
					msgpack_pack_str_body(&packer, key.c_str(), key.size());
				}

				// (1) - Value
				{
					msgpack_pack_nil(&packer);
				}
			}
		}

		auto header = rs485->makeHeader(target);
		
		vector<uint8_t> body;
		body.assign((uint8_t*)(messageBuffer.data)
			, (uint8_t*)(messageBuffer.data + messageBuffer.size));

		rs485->transmitHeaderAndBody(header, body);
		msgpack_sbuffer_destroy(&messageBuffer);
	}
}
