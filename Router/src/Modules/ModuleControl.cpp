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
		// Update values if local cache of what has been sent is different from parameters
		{
			if (this->debug.cachedValues.motorDriverSettings.current
				!= this->parameters.debug.motorDriverSettings.current) {
				this->setCurrent(this->parameters.debug.targetID.get()
					, this->parameters.debug.motorDriverSettings.current.get());
			}

			if (this->debug.cachedValues.motorDriverSettings.microstepResolution
				!= this->parameters.debug.motorDriverSettings.microstepResolution) {
				this->setMicrostepResolution(this->parameters.debug.targetID.get()
					, this->parameters.debug.motorDriverSettings.microstepResolution.get());
			}
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

		inspector->addButton("testTimer A", [this]() {
			this->runTestTimer(this->parameters.debug.targetID.get(), Axis::A);
			});

		inspector->addButton("testTimer B", [this]() {
			this->runTestTimer(this->parameters.debug.targetID.get(), Axis::B);
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
		ModuleControl::setMicrostepResolution(RS485::Target target, int value)
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
					string key = "setMicrostepResolution";
					msgpack_pack_str(&packer, key.size());
					msgpack_pack_str_body(&packer, key.c_str(), key.size());
				}

				// (1) - Value
				{
					msgpack_pack_uint8(&packer, log2(value));
				}
			}
		}

		auto header = rs485->makeHeader(target);

		vector<uint8_t> body;
		body.assign((uint8_t*)(messageBuffer.data)
			, (uint8_t*)(messageBuffer.data + messageBuffer.size));

		rs485->transmitHeaderAndBody(header, body);
		msgpack_sbuffer_destroy(&messageBuffer);

		this->debug.cachedValues.motorDriverSettings.microstepResolution = value;
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

	//---------
	void
		ModuleControl::runTestTimer(RS485::Target target, Axis axis)
	{
		auto rs485 = this->rs485.lock();
		if (!rs485) {
			ofLogError("No RS485");
			return;
		}

		auto period = this->parameters.debug.testTimer.period.get();
		auto count = this->parameters.debug.testTimer.count.get();

		if (this->parameters.debug.testTimer.normaliseParameters.get()) {
			auto microstepResolution = this->parameters.debug.motorDriverSettings.microstepResolution.get();
			period /= microstepResolution;
			count *= microstepResolution;
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
					string key = "testTimer";

					msgpack_pack_str(&packer, key.size());
					msgpack_pack_str_body(&packer, key.c_str(), key.size());
				}

				// (1) - Value
				{
					// Expecting an array [period_us, total_count]
					msgpack_pack_array(&packer, 2);
					msgpack_pack_uint32(&packer, period);
					msgpack_pack_uint32(&packer, count);
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
